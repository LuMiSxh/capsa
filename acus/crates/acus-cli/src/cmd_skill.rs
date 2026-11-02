// SPDX-License-Identifier: MPL-2.0
//! `acus self skill`: prints the agent skill, or installs it with `--install`.

use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::hint::ResultExt;
use capsa_core::skill::{Skill, Target};
use serde_json::Value;

/// The skill shipped with this binary, so hook output and installed copies match its version.
const SKILL: &str = include_str!("../../../skill/SKILL.md");
/// What the SessionStart hook injects into every session: the routing table and the patch
/// skeleton. The full SKILL.md stays one `acus self skill --full` away.
const CORE: &str = include_str!("../../../skill/CORE.md");

/// The skill this binary installs, for `capsa_core::skill`.
pub const SKILL_DEF: Skill = Skill {
    tool: "acus",
    text: SKILL,
};

#[derive(clap::Args)]
pub struct Args {
    /// Print the full skill instead of the short core the SessionStart hook injects.
    #[arg(long, conflicts_with = "install")]
    full: bool,
    /// Write SKILL.md to the skill directories of --target instead of printing it.
    #[arg(long)]
    install: bool,
    /// With --install: which agent's skill directory to write.
    #[arg(long, value_enum, default_value_t = Target::Claude, requires = "install")]
    target: Target,
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
    for file in capsa_core::skill::install(&home, &SKILL_DEF, a.target).hinted()? {
        println!("{}", file.display());
    }
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
}
