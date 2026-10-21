// SPDX-License-Identifier: MPL-2.0
//! `acus self skill`: prints, installs and removes the agent skill.

use crate::agents::Agent;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use serde_json::{Value, json};
use std::path::Path;

/// The skill shipped with this binary, so hook output and installed copies match its version.
const SKILL: &str = include_str!("../../../skill/SKILL.md");
/// What the SessionStart hook injects into every session: the routing table and the patch
/// skeleton. The full SKILL.md stays one `acus self skill --full` away.
const CORE: &str = include_str!("../../../skill/CORE.md");

#[derive(clap::Args)]
pub struct Args {
    /// Print the full skill instead of the short core the SessionStart hook injects.
    #[arg(long, conflicts_with = "install")]
    full: bool,
    /// Write SKILL.md to the skill directory of --agent instead of printing it.
    #[arg(long)]
    install: bool,
    /// With --install: which agent's skill directory to write.
    #[arg(long, value_enum, default_value_t = Agent::Claude, requires = "install")]
    agent: Agent,
    /// With --install and --agent claude or all: also register the SessionStart and PreToolUse hooks in ~/.claude/settings.json.
    #[arg(long, requires = "install")]
    hooks: bool,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, cfg: &crate::config::Config) -> Result<Outcome> {
    if !a.install {
        // A compaction or /clear ends the context the guard hints were shown in, so they show again.
        let hook = hook_input();
        if matches!(hook["source"].as_str(), Some("compact" | "clear")) {
            acus_guard::hints::reset(
                &crate::cmd_guard::guard_config(cfg),
                hook["session_id"].as_str().unwrap_or_default(),
            );
        }
        print!("{}", if a.full { body(SKILL) } else { CORE });
        return Ok(Outcome::Found);
    }
    let home = std::env::home_dir().context("no home directory")?;
    install(&home, a.agent, a.hooks)?;
    Ok(Outcome::Found)
}

/// The SessionStart hook's JSON on stdin (`session_id`, `source`). Null when stdin is a
/// terminal,
/// empty, not JSON or still open after a second, so the skill prints as before.
fn hook_input() -> Value {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return Value::Null;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::stdin().read_to_string(&mut text);
        let _ = tx.send(text);
    });
    rx.recv_timeout(std::time::Duration::from_secs(1))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// Writes the skill for `agent` under `home`, and the Claude hooks when `hooks` is set.
fn install(home: &Path, agent: Agent, hooks: bool) -> Result<()> {
    if hooks && agent == Agent::Codex {
        return Err(capsa_core::hint::Hinted::new(
            "--hooks registers Claude Code hooks",
            "use --agent claude or --agent all",
        )
        .into());
    }
    for dir in agent.dirs(home) {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let file = dir.join("SKILL.md");
        std::fs::write(&file, SKILL).with_context(|| format!("cannot write {}", file.display()))?;
        println!("{}", file.display());
    }
    if hooks {
        install_hooks(&home.join(".claude/settings.json"))?;
    }
    Ok(())
}

/// Removes the `SKILL.md` that `install` wrote for every agent, and its directory when that is
/// left empty. Returns the files removed, or with `dry_run` the files that would be.
#[cfg(feature = "cmd-update")]
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    for dir in Agent::All.dirs(home) {
        let file = dir.join("SKILL.md");
        if !file.is_file() {
            continue;
        }
        if !dry_run {
            std::fs::remove_file(&file)
                .with_context(|| format!("cannot remove {}", file.display()))?;
            // Only an empty directory goes; anything else in it is the user's.
            let _ = std::fs::remove_dir(&dir);
        }
        files.push(file);
    }
    Ok(files)
}

/// Registers the acus hooks in a Claude Code settings file, keeping a `.bak` of the old one.
fn install_hooks(path: &Path) -> Result<()> {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let settings = match &old {
        Some(t) => serde_json::from_str(t)
            .with_context(|| format!("{} is not valid JSON; fix it first", path.display()))?,
        None => json!({}),
    };
    if let Some(t) = &old {
        let bak = path.with_extension("json.bak");
        std::fs::write(&bak, t).with_context(|| format!("cannot write {}", bak.display()))?;
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(&merge_hooks(settings))?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}

/// Adds the acus SessionStart and PreToolUse hooks; earlier acus entries are replaced, so a
/// rerun never duplicates them and everything else in the settings stays as it was.
#[expect(
    clippy::unwrap_used,
    reason = "settings and hooks were made objects just above"
)]
fn merge_hooks(mut settings: Value) -> Value {
    const OURS: [(&str, &str, &str); 2] = [
        (
            "SessionStart",
            "startup|clear|compact",
            "acus self skill 2>/dev/null || true",
        ),
        (
            "PreToolUse",
            "Bash|Read|Grep|Glob|PowerShell",
            "acus guard 2>/dev/null || true",
        ),
    ];
    if !settings.is_object() {
        settings = json!({});
    }
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();
    let ours = |c: &Value| {
        c.as_str().is_some_and(|c| {
            c.contains("acus skill") || c.contains("acus self skill") || c.contains("acus guard")
        })
    };
    for groups in hooks.values_mut().filter_map(Value::as_array_mut) {
        groups.retain_mut(|g| {
            let Some(items) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = items.len();
            items.retain(|i| !ours(&i["command"]));
            !items.is_empty() || before == 0
        });
    }
    for (event, matcher, command) in OURS {
        let group = json!({"matcher": matcher, "hooks": [{"type": "command", "command": command}]});
        match hooks.entry(event).or_insert_with(|| json!([])) {
            Value::Array(a) => a.push(group),
            other => *other = json!([group]),
        }
    }
    settings
}

/// The skill without its YAML frontmatter, for injecting as session context.
fn body(s: &str) -> &str {
    let mut lines = s.split_inclusive('\n');
    let Some(first) = lines.next().filter(|l| l.trim_end() == "---") else {
        return s;
    };
    let mut end = first.len();
    for l in lines {
        end += l.len();
        if l.trim_end() == "---" {
            return s[end..].trim_start_matches(['\r', '\n']);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use crate::agents::{Agent, CLAUDE_DIR, CODEX_DIR};
    use serde_json::json;

    fn count(v: &serde_json::Value, event: &str) -> usize {
        v["hooks"][event].as_array().map_or(0, Vec::len)
    }

    #[test]
    fn hooks_into_empty_settings() {
        let v = super::merge_hooks(json!({}));
        assert_eq!(
            v["hooks"]["SessionStart"],
            json!([{"matcher": "startup|clear|compact", "hooks": [{"type": "command", "command": "acus self skill 2>/dev/null || true"}]}])
        );
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["matcher"],
            "Bash|Read|Grep|Glob|PowerShell"
        );
    }

    #[test]
    fn hooks_replace_the_old_skill_command() {
        let start = json!({"hooks": {"SessionStart": [
            {"matcher": "startup", "hooks": [{"type": "command", "command": "acus skill 2>/dev/null || true"}]}
        ]}});
        let v = super::merge_hooks(start);
        let g = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(
            g[0]["hooks"][0]["command"],
            "acus self skill 2>/dev/null || true"
        );
    }

    #[test]
    fn hooks_keep_unrelated_and_are_idempotent() {
        let start = json!({"model": "x", "hooks": {"PreToolUse": [
            {"matcher": "Edit", "hooks": [{"type": "command", "command": "fmt.sh"}]}
        ], "Stop": [{"hooks": []}]}, "theme": "dark"});
        let once = super::merge_hooks(start);
        assert_eq!(super::merge_hooks(once.clone()), once);
        assert_eq!(count(&once, "PreToolUse"), 2);
        assert_eq!(
            once["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "fmt.sh"
        );
        assert_eq!(once["hooks"]["Stop"], json!([{"hooks": []}]));
        let keys: Vec<_> = once.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "theme"]);
    }

    #[test]
    fn hooks_replace_old_guard_entry() {
        let start = json!({"hooks": {"PreToolUse": [
            {"matcher": "Bash", "hooks": [{"type": "command", "command": "acus guard"}]},
            {"matcher": "Bash", "hooks": [
                {"type": "command", "command": "acus guard"},
                {"type": "command", "command": "keep.sh"}
            ]}
        ]}});
        let v = super::merge_hooks(start);
        let g = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(
            g[0]["hooks"],
            json!([{"type": "command", "command": "keep.sh"}])
        );
        assert_eq!(g[1]["matcher"], "Bash|Read|Grep|Glob|PowerShell");
    }

    #[test]
    fn body_drops_frontmatter() {
        assert_eq!(super::body("---\nname: x\n---\n\n# acus\n"), "# acus\n");
        assert_eq!(super::body("# no frontmatter\n"), "# no frontmatter\n");
        assert!(super::body(super::SKILL).starts_with("# acus"));
    }

    #[test]
    fn core_stays_short_and_points_to_the_full_skill() {
        assert!(super::CORE.starts_with("# acus"));
        assert!(super::CORE.contains("acus self skill --full"));
        assert!(super::CORE.split_whitespace().count() < 250);
    }

    #[test]
    fn install_writes_only_the_chosen_agents_dirs() {
        for agent in [Agent::Claude, Agent::Codex, Agent::All] {
            let t = tempfile::tempdir().unwrap();
            super::install(t.path(), agent, false).unwrap();
            let written = |dir: &str| t.path().join(dir).join("SKILL.md").is_file();
            assert_eq!(written(CLAUDE_DIR), agent != Agent::Codex, "{agent:?}");
            assert_eq!(written(CODEX_DIR), agent != Agent::Claude, "{agent:?}");
            assert!(
                !t.path().join(".claude/settings.json").exists(),
                "{agent:?}"
            );
        }
    }

    #[test]
    fn hooks_need_a_claude_agent() {
        let t = tempfile::tempdir().unwrap();
        assert!(super::install(t.path(), Agent::Codex, true).is_err());
        assert!(!t.path().join(CODEX_DIR).exists());
        super::install(t.path(), Agent::Claude, true).unwrap();
        assert!(t.path().join(".claude/settings.json").is_file());
    }

    #[test]
    #[cfg(feature = "cmd-update")]
    fn uninstall_removes_only_the_skill_files() {
        let t = tempfile::tempdir().unwrap();
        super::install(t.path(), Agent::All, false).unwrap();
        let other = t.path().join(CODEX_DIR).join("notes.md");
        std::fs::write(&other, "mine").unwrap();
        let claude = t.path().join(CLAUDE_DIR).join("SKILL.md");

        let planned = super::uninstall(t.path(), true).unwrap();
        assert_eq!(planned.len(), 2);
        assert!(claude.is_file());

        let removed = super::uninstall(t.path(), false).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!t.path().join(CLAUDE_DIR).exists());
        assert!(other.is_file());
        assert!(!t.path().join(CODEX_DIR).join("SKILL.md").exists());
        assert!(super::uninstall(t.path(), false).unwrap().is_empty());
    }

    #[test]
    #[cfg(feature = "cmd-update")]
    fn update_reinstalls_where_the_skill_is_installed() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path();
        assert_eq!(crate::agents::installed_agent(h), Agent::Claude);
        std::fs::create_dir_all(h.join(CODEX_DIR)).unwrap();
        assert_eq!(crate::agents::installed_agent(h), Agent::Codex);
        std::fs::create_dir_all(h.join(CLAUDE_DIR)).unwrap();
        assert_eq!(crate::agents::installed_agent(h), Agent::All);
    }
}
