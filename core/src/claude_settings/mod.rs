// SPDX-License-Identifier: MPL-2.0
//! Reads and edits Claude Code's `settings.json`: one tool's hooks and the keys in `env`.
//! Everything else in the file stays as it was, and a write is refused when the file changed
//! since it was read.

use std::borrow::Cow;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::hint::Hint;

mod env;
#[cfg(test)]
mod tests;

pub use env::{Removal, env_value, remove_env, set_env};

const SETTINGS: &str = "settings.json";

/// A settings file as read, with the hook edits made since.
#[derive(Debug)]
pub struct Settings {
    /// The file that is written: the target of a symlink, so the link itself stays.
    pub path: PathBuf,
    /// The text that was read, or `None` when the file did not exist.
    before: Option<String>,
    /// The parsed settings, edited in place.
    pub value: Value,
}

/// What `set_hook` or `set_env` did to a tool's hook or env key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// The hook or env key was added.
    Install,
    /// The tool's entry or env value was replaced because it differed.
    Replace,
    /// The same entry is already there; nothing changes.
    Set,
}

impl Step {
    /// Whether applying this step changes the settings.
    pub fn writes(self) -> bool {
        !matches!(self, Self::Set)
    }

    /// The report word for this step; `dry_run` gives the wording for a run that writes nothing.
    pub fn text(self, dry_run: bool) -> &'static str {
        match (self, dry_run) {
            (Self::Install, false) => "installed",
            (Self::Install, true) => "would install",
            (Self::Replace, false) => "replaced",
            (Self::Replace, true) => "would replace",
            (Self::Set, _) => "already set",
        }
    }
}

/// Claude Code's settings file: `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
pub fn path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"))
        .join(SETTINGS)
}

/// Reads the settings at `path`. A missing file is an empty object; a symlink is followed, and
/// the target is what gets written.
///
/// # Errors
///
/// Fails when the file cannot be read, is not a JSON object, or has a malformed `hooks` or `env`.
pub fn read(path: &Path) -> Result<Settings, SettingsError> {
    let path = resolve(path)?;
    let Some(text) = read_text(&path)? else {
        return Ok(Settings {
            path,
            before: None,
            value: json!({}),
        });
    };
    let value: Value = serde_json::from_str(&text).map_err(|source| SettingsError::Parse {
        path: path.clone(),
        source,
    })?;
    if !value.is_object() {
        return Err(SettingsError::NotObject { path });
    }
    check_hooks(&value, &path)?;
    env::check_env(&value, &path)?;
    Ok(Settings {
        path,
        before: Some(text),
        value,
    })
}

/// Makes `event` run `command` under `matcher`, replacing any entry that `owns` claims.
///
/// An entry already identical is left alone and gives `Step::Set`. Other hooks in the event,
/// and entries that hold them, stay in place. `v` should come from `read`, which refuses
/// `hooks` of the wrong shape; other shapes are replaced.
pub(crate) fn set_hook(
    v: &mut Value,
    event: &str,
    matcher: Option<&str>,
    command: &str,
    owns: impl Fn(&str) -> bool,
) -> Step {
    let hook = json!({"type": "command", "command": command});
    if is_identical(event_entries(v, event), matcher, &hook, &owns) {
        return Step::Set;
    }
    let had = event_entries(v, event)
        .iter()
        .any(|g| items(g).any(|i| is_owned(i, &owns)));
    let mut entries = event_entries(v, event).to_vec();
    entries.retain_mut(|g| {
        let Some(items) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = items.len();
        items.retain(|i| !is_owned(i, &owns));
        // A group that held only our hooks goes; one that was empty already stays.
        !items.is_empty() || before == 0
    });
    entries.push(group(matcher, &hook));
    set_event_entries(v, event, entries);
    if had { Step::Replace } else { Step::Install }
}

/// Removes the entries of `event` that `owns` claims, with the groups and the event that they
/// leave empty. Other hooks stay in place. Returns whether an entry was removed.
pub(crate) fn remove_hook(v: &mut Value, event: &str, owns: impl Fn(&str) -> bool) -> bool {
    if !event_entries(v, event)
        .iter()
        .any(|g| items(g).any(|i| is_owned(i, &owns)))
    {
        return false;
    }
    let mut entries = event_entries(v, event).to_vec();
    entries.retain_mut(|g| {
        let Some(items) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = items.len();
        items.retain(|i| !is_owned(i, &owns));
        !items.is_empty() || before == 0
    });
    if entries.is_empty() {
        remove_event(v, event);
    } else {
        set_event_entries(v, event, entries);
    }
    true
}

/// Writes the settings over their file. The old file is copied to `<file>.bak` first. The new
/// text is staged beside the file with the old permissions, then renamed into place. Nothing is
/// replaced when the file no longer holds the text that `read` saw.
///
/// # Errors
///
/// Fails with `Changed` when the file changed since it was read, and with `Write` when a file
/// cannot be written. A failed write leaves the settings file as it was.
pub fn write(s: &Settings) -> Result<(), SettingsError> {
    let path = &s.path;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|source| SettingsError::Write {
        path: dir.to_owned(),
        source,
    })?;
    let mut text = serde_json::to_string_pretty(&s.value).map_err(SettingsError::Encode)?;
    text.push('\n');
    let old = std::fs::metadata(path).ok();
    if old.is_some() {
        let bak = suffixed(path, ".bak");
        std::fs::copy(path, &bak).map_err(|source| SettingsError::Write { path: bak, source })?;
    }
    // Not `fs::write_atomic`: it would not keep the file's mode, and the file can hold secrets in
    // `env`. The unchanged check runs after staging, so the time between it and the rename is one
    // rename.
    let tmp = suffixed(path, &format!(".capsa-tmp-{}", std::process::id()));
    let staged = stage(&tmp, &text, old.map(|m| m.permissions()))
        .map_err(|source| SettingsError::Write {
            path: tmp.clone(),
            source,
        })
        .and_then(|()| confirm_unchanged(s));
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).map_err(|source| {
        let _ = std::fs::remove_file(&tmp);
        SettingsError::Write {
            path: path.clone(),
            source,
        }
    })
}

/// The command Claude Code runs for `exe` with the arguments `sub`. A path with a space is
/// quoted. On Windows the path gets forward slashes, because Claude Code runs commands in Git
/// Bash, where a backslash is an escape.
pub fn command_line(exe: &Path, sub: &str) -> String {
    let exe = exe.display().to_string();
    if cfg!(windows) {
        format!("\"{}\" {sub}", exe.replace(std::path::MAIN_SEPARATOR, "/"))
    } else if exe.contains(' ') {
        format!("\"{exe}\" {sub}")
    } else {
        format!("{exe} {sub}")
    }
}

/// Whether `command` runs `binary` with `sub`, in the plain, quoted or `.exe` form. Useful in the
/// `owns` claims of a `HookSpec`.
pub fn runs(command: &str, binary: &str, sub: &str) -> bool {
    let plain = command
        .replace(['"', '\''], "")
        .replace(".exe", "")
        .replace(".EXE", "");
    let needle = format!("{binary} {sub}");
    plain.match_indices(&needle).any(|(at, _)| {
        plain[..at]
            .chars()
            .next_back()
            .is_none_or(|c| matches!(c, ' ' | '/' | '\\' | '$'))
    })
}

/// The settings file that `path` names, following a symlink to its target.
fn resolve(path: &Path) -> Result<PathBuf, SettingsError> {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return Ok(path.to_owned());
    }
    std::fs::canonicalize(path).map_err(|source| SettingsError::Symlink {
        path: path.to_owned(),
        source,
    })
}

/// Refuses `hooks` that is not an object, or an event that is not an array.
fn check_hooks(value: &Value, path: &Path) -> Result<(), SettingsError> {
    let shape = |key: String, expected: &'static str| SettingsError::Shape {
        path: path.to_owned(),
        key,
        expected,
    };
    let Some(hooks) = value.get("hooks") else {
        return Ok(());
    };
    let Some(events) = hooks.as_object() else {
        return Err(shape("hooks".to_owned(), "an object"));
    };
    match events.iter().find(|(_, groups)| !groups.is_array()) {
        Some((event, _)) => Err(shape(format!("hooks.{event}"), "an array")),
        None => Ok(()),
    }
}

pub(crate) fn event_entries<'a>(v: &'a Value, event: &str) -> &'a [Value] {
    v.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// Replaces the entries of `event` under `hooks` with `entries`, creating what is missing.
pub(crate) fn set_event_entries(v: &mut Value, event: &str, entries: Vec<Value>) {
    let mut hooks = take_object(v, "hooks");
    hooks.insert(event.to_owned(), Value::Array(entries));
    put_key(v, "hooks", Value::Object(hooks));
}

/// Drops `event` from `hooks`, and `hooks` when nothing is left in it.
fn remove_event(v: &mut Value, event: &str) {
    let mut hooks = take_object(v, "hooks");
    hooks.remove(event);
    if hooks.is_empty() {
        if let Some(root) = v.as_object_mut() {
            root.remove("hooks");
        }
    } else {
        put_key(v, "hooks", Value::Object(hooks));
    }
}

/// Sets `key` in the object `v`, replacing `v` when it is not an object.
pub(crate) fn put_key(v: &mut Value, key: &str, value: Value) {
    match v.as_object_mut() {
        Some(root) => {
            root.insert(key.to_owned(), value);
        }
        None => *v = json!({ key: value }),
    }
}

/// Takes the object under `key` out of `v`, leaving an empty one in place, so that it can be
/// edited and given back with `put_key`. A value of another shape gives an empty object.
fn take_object(v: &mut Value, key: &str) -> Map<String, Value> {
    v.get_mut(key)
        .and_then(Value::as_object_mut)
        .map(std::mem::take)
        .unwrap_or_default()
}

pub(crate) fn items(group: &Value) -> impl Iterator<Item = &Value> + '_ {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

pub(crate) fn matcher_of(group: &Value) -> Option<&str> {
    group.get("matcher").and_then(Value::as_str)
}

fn is_owned(item: &Value, owns: &impl Fn(&str) -> bool) -> bool {
    item.get("command")
        .and_then(Value::as_str)
        .is_some_and(owns)
}

/// Whether the tool has exactly one hook here, under `matcher`, and it equals `hook`.
fn is_identical(
    entries: &[Value],
    matcher: Option<&str>,
    hook: &Value,
    owns: &impl Fn(&str) -> bool,
) -> bool {
    let owned = entries
        .iter()
        .flat_map(items)
        .filter(|i| is_owned(i, owns))
        .count();
    owned == 1
        && entries
            .iter()
            .any(|g| matcher_of(g) == matcher && items(g).any(|i| i == hook))
}

pub(crate) fn group(matcher: Option<&str>, hook: &Value) -> Value {
    let mut group = Map::new();
    if let Some(matcher) = matcher {
        group.insert("matcher".to_owned(), json!(matcher));
    }
    group.insert("hooks".to_owned(), json!([hook]));
    Value::Object(group)
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes `text` to `tmp` with `perms` when given, then syncs it to disk.
fn stage(tmp: &Path, text: &str, perms: Option<std::fs::Permissions>) -> std::io::Result<()> {
    let mut file = std::fs::File::create(tmp)?;
    file.write_all(text.as_bytes())?;
    if let Some(perms) = perms {
        file.set_permissions(perms)?;
    }
    file.sync_all()
}

/// Reads the text of `path`, or `None` when the file does not exist.
fn read_text(path: &Path) -> Result<Option<String>, SettingsError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SettingsError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Fails unless the file on disk still holds the text that `read` returned.
fn confirm_unchanged(s: &Settings) -> Result<(), SettingsError> {
    let now = read_text(&s.path)?;
    if now == s.before {
        return Ok(());
    }
    let change = match (&s.before, &now) {
        (None, Some(_)) => Change::Created,
        (Some(_), None) => Change::Removed,
        _ => Change::Edited,
    };
    Err(SettingsError::Changed {
        path: s.path.clone(),
        change,
    })
}

/// How the settings file differs from the text that `read` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The file did not exist when it was read and exists now.
    Created,
    /// The file existed when it was read and is gone now.
    Removed,
    /// The file holds other text than when it was read.
    Edited,
}

/// Why the settings file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("cannot follow {}", path.display())]
    Symlink {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not valid JSON", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{} is not a JSON object", path.display())]
    NotObject { path: PathBuf },
    #[error("{key} in {} must be {expected}", path.display())]
    Shape {
        path: PathBuf,
        key: String,
        expected: &'static str,
    },
    #[error("{} changed while writing", path.display())]
    Changed { path: PathBuf, change: Change },
    #[error("cannot write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot encode the settings")]
    Encode(#[source] serde_json::Error),
}

impl Hint for SettingsError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Self::Symlink { .. } => Cow::Borrowed(
                "point the link at an existing file, or remove it; nothing was written",
            ),
            Self::Parse { path, .. } | Self::NotObject { path } | Self::Shape { path, .. } => {
                Cow::Owned(format!(
                    "fix {} by hand and rerun; nothing was written",
                    path.display()
                ))
            }
            Self::Changed { .. } => Cow::Borrowed("run again; nothing was replaced"),
            Self::Read { .. } | Self::Write { .. } | Self::Encode(_) => {
                Cow::Borrowed("check the file permissions")
            }
        })
    }
}
