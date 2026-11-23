// SPDX-License-Identifier: MPL-2.0
//! Claude Code's permission rules for the calls a tool makes for its user: shell commands, file
//! reads and edits, fetches and MCP tools. The rules come from the same files Claude Code reads.
//! A tool that cannot prompt refuses a call that a rule denies or asks about; a call no rule names
//! goes ahead, unless the permission mode (see [`Mode`]) says otherwise.
//!
//! Claude Code evaluates deny, then ask, then allow, across every settings file, so a deny in one
//! file beats an allow in another and no file order matters. Allow rules matter only where the
//! mode would make Claude Code refuse a call that no allow rule covers: `dontAsk`, and the other
//! modes with `strict`.
//!
//! Callers: `acus batch` checks the `Bash(...)` rules; `acus script` checks every kind.

mod bash;
mod fetch;
mod mcp;
pub mod mode;
mod path;
mod policy;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mode;

pub use mode::{Mode, ModeStore};

use crate::claude_settings;
use policy::Scope;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A call a rule refuses, or a settings file that cannot be read: the message and the fix.
#[derive(Debug)]
pub struct Refusal {
    pub message: String,
    pub hint: Option<String>,
}

impl Refusal {
    fn new(message: impl Into<String>) -> Self {
        Refusal {
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Which list of the settings a rule is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Deny,
    Ask,
    Allow,
}

/// A rule as written in the settings, with where it came from.
pub(super) struct Entry {
    text: String,
    rule: Rule,
    file: PathBuf,
}

/// The kind of call a rule covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Bash,
    Read,
    Edit,
    Fetch,
    Mcp,
}

impl Entry {
    /// Splits `Name(spec)` into its name and spec; a rule with no parentheses has no spec and
    /// covers every call of its tool.
    fn parts(&self) -> (&str, Option<&str>) {
        if let Some((name, rest)) = self.text.split_once('(')
            && let Some(spec) = rest.strip_suffix(')')
        {
            return (name, Some(spec));
        }
        (&self.text, None)
    }

    /// `Write` and the other editing tools share the `Edit` rules, as in Claude Code.
    fn tool(&self) -> Option<Tool> {
        match self.parts().0 {
            "Bash" => Some(Tool::Bash),
            "Read" => Some(Tool::Read),
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Some(Tool::Edit),
            "WebFetch" => Some(Tool::Fetch),
            _ => None,
        }
    }

    /// Returns the rule's spec when the rule covers `tool`; the spec is `None` when the rule
    /// covers every call of the tool.
    fn spec_of(&self, tool: Tool) -> Option<Option<&str>> {
        (self.tool() == Some(tool)).then(|| self.parts().1)
    }

    fn refusal(&self, what: &str) -> Refusal {
        let (why, key, hint) = if self.rule == Rule::Ask {
            (
                "needs approval",
                "ask",
                "a script cannot prompt; run it as its own tool call so Claude Code asks, or remove the rule",
            )
        } else {
            (
                "is denied",
                "deny",
                "scripts follow the deny rules of the conversation; remove or narrow the rule to allow it",
            )
        };
        Refusal::new(format!(
            "{what} {why} by `{}` in permissions.{key} ({})",
            self.text,
            self.file.display()
        ))
        .with_hint(hint)
    }
}

/// Decides each call a tool makes against the rules of the managed, user, project and local
/// settings, and the permission mode. Files are read on the first decision, so a tool that makes
/// no checked call reads nothing.
pub struct Permissions {
    root: PathBuf,
    cwd: PathBuf,
    home: Option<PathBuf>,
    files: Vec<PathBuf>,
    entries: OnceLock<Result<Vec<Entry>, String>>,
    mode: Option<Mode>,
    strict: bool,
}

impl Permissions {
    /// Returns the rules for a tool running in `cwd`; the project is the enclosing git
    /// repository, or `cwd` outside one.
    pub fn new(cwd: PathBuf) -> Self {
        let root = cwd
            .ancestors()
            .find(|dir| dir.join(".git").exists())
            .unwrap_or(&cwd)
            .to_path_buf();
        let files = vec![
            managed_path(),
            claude_settings::path(),
            root.join(".claude/settings.json"),
            root.join(".claude/settings.local.json"),
        ];
        Self::with_files(files, root, cwd, std::env::home_dir())
    }

    pub fn with_files(
        files: Vec<PathBuf>,
        root: PathBuf,
        cwd: PathBuf,
        home: Option<PathBuf>,
    ) -> Self {
        Permissions {
            root,
            cwd,
            home,
            files,
            entries: OnceLock::new(),
            mode: None,
            strict: false,
        }
    }

    /// Sets the permission mode in force; `None`, the mode not being known, keeps the rules as
    /// they were before modes: only deny and ask rules refuse.
    #[must_use]
    pub fn with_mode(mut self, mode: Option<Mode>) -> Self {
        self.mode = mode;
        self
    }

    /// Makes every mode that would prompt for a call refuse it unless an allow rule covers it,
    /// since a tool cannot prompt. `bypassPermissions`, which never prompts, is unaffected.
    #[must_use]
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Returns the project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the directories that `permissions.additionalDirectories` adds to the project, with
    /// `~/` expanded and relative paths taken from the project root.
    pub fn additional_dirs(&self) -> Result<Vec<PathBuf>, Refusal> {
        let mut dirs = Vec::new();
        for file in &self.files {
            let settings = claude_settings::read(file)
                .map_err(|e| Refusal::new(format!("cannot read {}: {e}", file.display())))?;
            let listed = settings.value["permissions"]["additionalDirectories"].as_array();
            for text in listed.into_iter().flatten().filter_map(|e| e.as_str()) {
                let dir = match text.strip_prefix("~/") {
                    Some(rest) => match &self.home {
                        Some(home) => home.join(rest),
                        None => continue,
                    },
                    None => self.root.join(text),
                };
                dirs.push(dir);
            }
        }
        Ok(dirs)
    }

    /// Refuses a shell command that a `Bash(...)` rule denies or asks about. Every subcommand of a
    /// compound command is checked.
    pub fn check_bash(&self, command: &str) -> Result<(), Refusal> {
        self.refuse(
            Tool::Bash,
            &format!("the command `{command}`"),
            Scope::Elsewhere,
            |spec| spec.is_none_or(|s| bash::matches(s, command)),
            |specs| specs.contains(&None) || bash::covered(specs, command),
        )
    }

    /// Refuses reading `path` when a `Read(...)` rule denies or asks about it.
    pub fn check_read(&self, path: &Path) -> Result<(), Refusal> {
        self.check_path(Tool::Read, "reading", path)
    }

    /// Refuses changing `path` when an `Edit(...)` or `Write(...)` rule denies or asks about it.
    pub fn check_edit(&self, path: &Path) -> Result<(), Refusal> {
        self.check_path(Tool::Edit, "editing", path)
    }

    /// Refuses fetching `url` when a `WebFetch(domain:...)` rule denies or asks about its host.
    pub fn check_fetch(&self, url: &str) -> Result<(), Refusal> {
        let host = fetch::host(url);
        let host = host.as_deref();
        self.refuse(
            Tool::Fetch,
            &format!("fetching `{url}`"),
            Scope::Elsewhere,
            |spec| spec.is_none_or(|s| host.is_some_and(|h| fetch::matches(s, h))),
            |specs| {
                specs
                    .iter()
                    .any(|spec| spec.is_none_or(|s| host.is_some_and(|h| fetch::matches(s, h))))
            },
        )
    }

    fn check_path(&self, tool: Tool, verb: &str, path: &Path) -> Result<(), Refusal> {
        let targets = path::targets(path, &self.cwd);
        let place = path::Place::new(&self.root, &self.cwd, self.home.as_deref());
        let scope = if tool == Tool::Edit && self.protected(&targets) {
            Scope::Protected
        } else if targets.iter().all(|t| place.in_cwd(t)) {
            Scope::Local
        } else {
            Scope::Elsewhere
        };
        self.refuse(
            tool,
            &format!("{verb} `{}`", path.display()),
            scope,
            |spec| spec.is_none_or(|s| targets.iter().any(|t| path::matches(s, t, &place))),
            |specs| {
                specs.iter().any(|spec| {
                    spec.is_none_or(|s| targets.iter().all(|t| path::matches(s, t, &place)))
                })
            },
        )
    }

    /// Returns whether an edit of `targets` reaches `.git/` or a settings file the rules come from.
    fn protected(&self, targets: &[String]) -> bool {
        let settings: Vec<String> = self
            .files
            .iter()
            .flat_map(|f| path::targets(f, &self.cwd))
            .collect();
        targets
            .iter()
            .any(|t| path::in_git_dir(t) || settings.contains(t))
    }

    /// Returns the error for the first deny rule of `tool` that `hit` accepts, else for the first
    /// ask rule, else the mode's refusal of a call that `allowed` finds no allow rule for. `local`
    /// says where the call reaches.
    fn refuse(
        &self,
        tool: Tool,
        what: &str,
        scope: Scope,
        hit: impl Fn(Option<&str>) -> bool,
        allowed: impl FnOnce(&[Option<&str>]) -> bool,
    ) -> Result<(), Refusal> {
        let entries = self.entries()?;
        for rule in [Rule::Deny, Rule::Ask] {
            let found = entries
                .iter()
                .find(|e| e.rule == rule && e.spec_of(tool).is_some_and(&hit));
            if let Some(entry) = found {
                return Err(entry.refusal(what));
            }
        }
        self.gate(tool, scope, what, || {
            let specs: Vec<_> = entries
                .iter()
                .filter(|e| e.rule == Rule::Allow)
                .filter_map(|e| e.spec_of(tool))
                .collect();
            allowed(&specs)
        })
    }

    fn entries(&self) -> Result<&[Entry], Refusal> {
        match self.entries.get_or_init(|| load(&self.files)) {
            Ok(entries) => Ok(entries),
            Err(message) => Err(Refusal::new(message.clone()).with_hint(
                "fix the settings file; scripts refuse calls while the permission rules cannot be read",
            )),
        }
    }
}

/// Returns the deny and ask rules of `files`. A settings file that does not parse is an error,
/// because ignoring it could let a denied call run.
fn load(files: &[PathBuf]) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for file in files {
        let settings = claude_settings::read(file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        for (key, rule) in [
            ("deny", Rule::Deny),
            ("ask", Rule::Ask),
            ("allow", Rule::Allow),
        ] {
            let rules = settings.value["permissions"][key].as_array();
            for text in rules.into_iter().flatten().filter_map(|e| e.as_str()) {
                entries.push(Entry {
                    text: text.to_owned(),
                    rule,
                    file: file.clone(),
                });
            }
        }
    }
    Ok(entries)
}

/// Returns where the organization's managed settings live. Debug builds, which the tests use, read
/// `ACUS_MANAGED_SETTINGS` instead; a release build has no override, so an agent cannot move the
/// file it must obey.
fn managed_path() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("ACUS_MANAGED_SETTINGS") {
        return PathBuf::from(path);
    }
    let path = if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode/managed-settings.json"
    } else if cfg!(windows) {
        r"C:\Program Files\ClaudeCode\managed-settings.json"
    } else {
        "/etc/claude-code/managed-settings.json"
    };
    PathBuf::from(path)
}
