// SPDX-License-Identifier: MPL-2.0
//! `census self setup`: installs the skill, then adds census's statusLine and hooks to Claude Code's
//! `settings.json`. Entries that are there already stay as they are. The file is written only when
//! something changes, and the old one is copied to `settings.json.bak` first.

use crate::cmd_self::{SKILL, install_skill, skill_file};
use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::Hinted;
use census::context::claude_config_dir;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The settings file under the configuration directory.
const SETTINGS: &str = "settings.json";
/// The copy of the settings file from before a change.
const BACKUP: &str = "settings.json.bak";
/// The events that get census's hook, in the order they are reported.
const EVENTS: [&str; 2] = ["UserPromptSubmit", "PostToolUse"];

/// The hint when the settings file cannot be changed.
const FIX: &str = "fix settings.json by hand and rerun; nothing was written";

#[derive(clap::Args)]
pub struct Args {
    /// Reports what would change and writes nothing, the skill included.
    #[arg(long)]
    dry_run: bool,
    /// Replaces a statusLine that census does not own.
    #[arg(long)]
    force: bool,
}

/// What an item already holds before setup changes it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Found {
    Missing,
    Set,
    Foreign,
}

/// What setup does with an item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Install,
    Replace,
    Set,
    Skip,
}

impl Step {
    fn decide(found: Found, force: bool) -> Self {
        match (found, force) {
            (Found::Missing, _) => Self::Install,
            (Found::Set, _) => Self::Set,
            (Found::Foreign, true) => Self::Replace,
            (Found::Foreign, false) => Self::Skip,
        }
    }

    /// Whether this step writes the item.
    fn writes(self) -> bool {
        matches!(self, Self::Install | Self::Replace)
    }

    fn text(self, dry_run: bool) -> &'static str {
        match (self, dry_run) {
            (Self::Set, _) => "already set",
            (Self::Install, false) => "installed",
            (Self::Install, true) => "would install",
            (Self::Replace, false) => "installed (replaced foreign statusLine)",
            (Self::Replace, true) => "would install (replaces foreign statusLine)",
            (Self::Skip, _) => "skipped (foreign statusLine; --force replaces it)",
        }
    }
}

/// One line of the report.
#[derive(Serialize)]
struct Item {
    name: String,
    status: &'static str,
}

#[derive(Serialize)]
struct Report {
    dry_run: bool,
    settings: String,
    items: Vec<Item>,
}

/// Installs the skill and wires census into the settings, then prints what it did.
pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let exe = std::env::current_exe().context("cannot find the path of this binary")?;
    let dir = claude_config_dir();
    let path = settings_target(&dir.join(SETTINGS))?;
    let (before, planned) = read_settings(&path)?;
    let mut after = before.clone();
    let skill = if skill_is_current(&home) {
        Step::Set
    } else {
        Step::Install
    };
    let mut steps = vec![("skill".to_owned(), skill)];
    steps.extend(wire(&mut after, &exe, a.force)?);
    if !a.dry_run {
        if skill.writes() {
            install_skill(&home)?;
        }
        if after != before {
            write_settings(&path, &after, planned.as_deref())?;
        }
    }
    let report = Report {
        dry_run: a.dry_run,
        settings: dir.join(SETTINGS).display().to_string(),
        items: steps
            .into_iter()
            .map(|(name, step)| Item {
                name,
                status: step.text(a.dry_run),
            })
            .collect(),
    };
    print_report(&report, out)?;
    Ok(Outcome::Found)
}

fn skill_is_current(home: &Path) -> bool {
    std::fs::read_to_string(skill_file(home)).is_ok_and(|text| text == SKILL)
}

/// The file that census writes for `link`: the file a symlink points to, so the link stays.
fn settings_target(link: &Path) -> Result<PathBuf> {
    let is_link = std::fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return Ok(link.to_owned());
    }
    match std::fs::canonicalize(link) {
        Ok(target) => Ok(target),
        Err(e) => Err(Hinted::new(
            format!("cannot follow {}: {e}", link.display()),
            "point the link at an existing file, or remove it; nothing was written",
        )
        .into()),
    }
}

/// Reads the settings object and the text it came from. A missing file is an empty object with no
/// text; anything else that is not a JSON object is an error.
fn read_settings(path: &Path) -> Result<(Map<String, Value>, Option<String>)> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Map::new(), None)),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(settings)) => Ok((settings, Some(text))),
        Ok(_) => Err(Hinted::new(format!("{} is not a JSON object", path.display()), FIX).into()),
        Err(e) => {
            Err(Hinted::new(format!("{} is not valid JSON: {e}", path.display()), FIX).into())
        }
    }
}

/// Sets census's statusLine and adds its hook to each event, returning what each item did.
fn wire(settings: &mut Map<String, Value>, exe: &Path, force: bool) -> Result<Vec<(String, Step)>> {
    let mut hooks = match settings.get("hooks") {
        None => Map::new(),
        Some(Value::Object(hooks)) => hooks.clone(),
        Some(_) => {
            return Err(Hinted::new("hooks in settings.json is not a JSON object", FIX).into());
        }
    };
    let line = census_command(exe, "statusline --line");
    let mut steps = vec![(
        "statusLine".to_owned(),
        wire_status_line(settings, &line, force),
    )];
    let hook = census_command(exe, "hook");
    for event in EVENTS {
        let step = wire_hook(&mut hooks, event, &hook)?;
        steps.push((format!("hook {event}"), step));
    }
    settings.insert("hooks".to_owned(), Value::Object(hooks));
    Ok(steps)
}

fn wire_status_line(settings: &mut Map<String, Value>, line: &str, force: bool) -> Step {
    let found = match settings.get("statusLine") {
        None => Found::Missing,
        Some(value) if command_of(value).is_some_and(|c| runs_census(c, "statusline")) => {
            Found::Set
        }
        Some(_) => Found::Foreign,
    };
    let step = Step::decide(found, force);
    if step.writes() {
        settings.insert(
            "statusLine".to_owned(),
            json!({"type": "command", "command": line}),
        );
    }
    step
}

/// Appends census's hook entry to `event` unless one of its commands already runs census's hook.
fn wire_hook(hooks: &mut Map<String, Value>, event: &str, command: &str) -> Result<Step> {
    let mut entries = match hooks.get(event) {
        None => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(_) => {
            return Err(Hinted::new(
                format!("hooks.{event} in settings.json is not a JSON array"),
                FIX,
            )
            .into());
        }
    };
    let found = if entries.iter().any(runs_census_hook) {
        Found::Set
    } else {
        Found::Missing
    };
    let step = Step::decide(found, false);
    if step.writes() {
        entries.push(json!({"hooks": [{"type": "command", "command": command}]}));
        hooks.insert(event.to_owned(), Value::Array(entries));
    }
    Ok(step)
}

/// The command Claude Code runs: this binary, quoted when its path has a space, then `sub`.
/// On Windows the path always gets forward slashes and quotes: Claude Code runs commands in Git
/// Bash, where a backslash is an escape.
fn census_command(exe: &Path, sub: &str) -> String {
    let exe = exe.display().to_string();
    if cfg!(windows) {
        format!("\"{}\" {sub}", exe.replace(std::path::MAIN_SEPARATOR, "/"))
    } else if exe.contains(' ') {
        format!("\"{exe}\" {sub}")
    } else {
        format!("{exe} {sub}")
    }
}

fn command_of(entry: &Value) -> Option<&str> {
    entry.get("command")?.as_str()
}

/// Whether a command runs census's `sub` subcommand. Matches the plain form, the quoted form that
/// a path with a space gets, and the `.exe` form on Windows.
fn runs_census(command: &str, sub: &str) -> bool {
    let plain = command
        .replace('"', "")
        .replace(".exe", "")
        .replace(".EXE", "");
    plain.contains(&format!("census {sub}"))
}

/// Whether a hook entry has a command that runs census's hook.
fn runs_census_hook(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|hook| command_of(hook).is_some_and(|c| runs_census(c, "hook")))
        })
}

/// Writes `settings` over `path`, the resolved settings file. The text goes to a temporary file
/// beside it, which gets the old file's permissions and is synced before it is renamed into place.
/// The old file is copied to `settings.json.bak` first. Nothing is replaced when the file no
/// longer holds `planned`, the text the settings were planned from.
fn write_settings(path: &Path, settings: &Map<String, Value>, planned: Option<&str>) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut text = serde_json::to_string_pretty(settings)?;
    text.push('\n');
    let old = std::fs::metadata(path).ok();
    if old.is_some() {
        let backup = dir.join(BACKUP);
        std::fs::copy(path, &backup)
            .with_context(|| format!("cannot write {}", backup.display()))?;
    }
    let tmp = dir.join(format!("{SETTINGS}.census-tmp-{}", std::process::id()));
    let staged = stage(&tmp, &text, old.map(|m| m.permissions()))
        .with_context(|| format!("cannot write {}", tmp.display()))
        .and_then(|()| confirm_unchanged(path, planned));
    if staged.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    staged?;
    let renamed = std::fs::rename(&tmp, path);
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    renamed.with_context(|| format!("cannot replace {}", path.display()))
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

/// Fails when `path` no longer holds `planned`, so that a write which raced another one is refused.
fn confirm_unchanged(path: &Path, planned: Option<&str>) -> Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() == planned {
        return Ok(());
    }
    Err(Hinted::new(
        format!("{} changed while writing", path.display()),
        "run again; nothing was replaced",
    )
    .into())
}

/// Prints the report: one line per item, or the JSON document.
fn print_report(report: &Report, out: &Out) -> Result<()> {
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    for item in &report.items {
        println!("{}", out.field(&format!("{:<24}", item.name), item.status));
    }
    println!(
        "{}",
        out.field(&format!("{:<24}", "settings"), &report.settings)
    );
    if report.dry_run {
        println!("dry run: nothing was written");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quoted_and_exe_forms_of_census_commands_are_recognised() {
        assert!(runs_census(r#""/My Apps/census" hook"#, "hook"));
        assert!(runs_census(
            r#""C:\Program Files\census.exe" statusline --line"#,
            "statusline"
        ));
        assert!(runs_census(
            r#"census hook 2>/dev/null || "$HOME/.cargo/bin/census" hook || true"#,
            "hook"
        ));
        assert!(!runs_census("my-line", "statusline"));
        assert!(!runs_census("census statusline --line", "hook"));
    }

    #[test]
    fn a_file_that_changed_since_it_was_read_is_not_replaced() {
        let dir = std::env::temp_dir().join(format!("census-setup-{}-changed", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        let path = dir.join(SETTINGS);
        std::fs::write(&path, r#"{"theme":"light"}"#).expect("write the settings");
        let result = write_settings(&path, &Map::new(), Some(r#"{"theme":"dark"}"#));
        let kept = std::fs::read_to_string(&path).expect("read the settings");
        let left_behind = std::fs::read_dir(&dir)
            .expect("list the scratch directory")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains("census-tmp"));
        let _ = std::fs::remove_dir_all(&dir);
        let err = result.expect_err("a changed file is refused");
        assert!(
            format!("{err:#}").contains("changed while writing"),
            "{err:#}"
        );
        assert_eq!(kept, r#"{"theme":"light"}"#);
        assert!(!left_behind);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_binary_path_with_a_space_is_quoted() {
        assert_eq!(
            census_command(Path::new("/usr/bin/census"), "hook"),
            "/usr/bin/census hook"
        );
        assert_eq!(
            census_command(Path::new("/My Apps/census"), "hook"),
            "\"/My Apps/census\" hook"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_binary_path_is_quoted_with_forward_slashes_and_still_recognised() {
        let command = census_command(Path::new(r"C:\My Apps\census.exe"), "hook");
        assert_eq!(command, r#""C:/My Apps/census.exe" hook"#);
        assert!(runs_census(&command, "hook"));
    }
}
