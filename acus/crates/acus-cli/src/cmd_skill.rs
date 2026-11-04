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

/// A long-form reference topic, so flag lists stay out of the skill an agent preloads.
#[derive(Clone, Copy, clap::ValueEnum)]
enum Topic {
    Find,
    Read,
    Patch,
    Git,
    Ctx,
}

impl Topic {
    const fn text(self) -> &'static str {
        match self {
            Self::Find => include_str!("../../../skill/reference/find.md"),
            Self::Read => include_str!("../../../skill/reference/read.md"),
            Self::Patch => include_str!("../../../skill/reference/patch.md"),
            Self::Git => include_str!("../../../skill/reference/git.md"),
            Self::Ctx => include_str!("../../../skill/reference/ctx.md"),
        }
    }
}

/// The reference files installed beside `SKILL.md`, by path below the skill directory.
const REFERENCES: [(&str, &str); 5] = [
    ("reference/find.md", Topic::Find.text()),
    ("reference/read.md", Topic::Read.text()),
    ("reference/patch.md", Topic::Patch.text()),
    ("reference/git.md", Topic::Git.text()),
    ("reference/ctx.md", Topic::Ctx.text()),
];

/// The skill this binary installs, for `capsa_core::skill`.
pub const SKILL_DEF: Skill = Skill {
    tool: "acus",
    text: SKILL,
    extra: &REFERENCES,
};

#[derive(clap::Args)]
pub struct Args {
    /// Print the reference for this topic instead of the core skill.
    #[arg(long, value_enum, conflicts_with_all = ["install", "full"])]
    topic: Option<Topic>,
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
    if let Some(topic) = a.topic {
        outprint!("{}", topic.text());
        return Ok(Outcome::Found);
    }
    if !a.install {
        // A compaction or /clear ends the context the guard hints were shown in, so they show again.
        let hook = hook_input();
        if matches!(hook["source"].as_str(), Some("compact" | "clear")) {
            acus_guard::hints::reset(
                &crate::cmd_guard::guard_config(cfg),
                hook["session_id"].as_str().unwrap_or_default(),
            );
        }
        outprint!("{}", if a.full { body(SKILL) } else { CORE });
        return Ok(Outcome::Found);
    }
    let home = std::env::home_dir().context("no home directory")?;
    let written = capsa_core::skill::install(&home, &SKILL_DEF, a.target).hinted()?;
    for file in written {
        outln!("{}", file.display());
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
    use clap::CommandFactory;
    use std::collections::BTreeSet;

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

    /// The frontmatter keys and values, which must be `name` and `description` only.
    fn frontmatter(text: &str) -> Vec<(&str, &str)> {
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("---"));
        let mut fields = Vec::new();
        for line in lines {
            if line == "---" {
                return fields;
            }
            let (key, value) = line.split_once(':').expect("key: value");
            fields.push((key.trim(), value.trim()));
        }
        panic!("unterminated frontmatter");
    }

    #[test]
    fn frontmatter_has_the_installed_name_and_one_description() {
        let fields = frontmatter(super::SKILL);
        assert_eq!(
            fields.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["name", "description"]
        );
        assert_eq!(fields[0].1, super::SKILL_DEF.tool);
        assert!(fields[1].1.len() <= 1024, "{} chars", fields[1].1.len());
        assert!(super::SKILL.lines().count() < 500);
    }

    #[test]
    fn core_is_a_subset_of_the_full_skill() {
        let skill: BTreeSet<&str> = super::SKILL
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        for line in super::CORE
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            assert!(skill.contains(line), "line missing from SKILL.md: {line}");
        }
    }

    /// Every `acus <command> …` mention in a code span or fenced block, with its flags.
    fn mentioned_commands(text: &str) -> Vec<(String, Option<String>, Vec<String>)> {
        let mut found = Vec::new();
        let mut fenced = false;
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                if let Some(rest) = line.trim_start().strip_prefix("acus ") {
                    found.push(parse_mention(rest));
                }
                continue;
            }
            for span in line.split('`').skip(1).step_by(2) {
                for part in span.split("acus ").skip(1) {
                    found.push(parse_mention(part));
                }
            }
        }
        found
    }

    fn parse_mention(text: &str) -> (String, Option<String>, Vec<String>) {
        let tokens: Vec<&str> = text
            .split_whitespace()
            .map(|token| token.trim_matches(|c: char| "[](),;:.\"'".contains(c)))
            .filter(|token| !token.is_empty())
            .collect();
        let Some(sub) = tokens.first() else {
            return (String::new(), None, Vec::new());
        };
        let action = (*sub == "self" && tokens.get(1).is_some_and(|t| !t.starts_with('-')))
            .then(|| tokens[1].to_owned());
        let start = if action.is_some() { 2 } else { 1 };
        let flags = tokens[start..]
            .iter()
            .flat_map(|token| token.split('/'))
            .filter_map(flag)
            .collect();
        ((*sub).to_owned(), action, flags)
    }

    fn flag(token: &str) -> Option<String> {
        if let Some(long) = token.strip_prefix("--") {
            let long = long.split_once('=').map_or(long, |(name, _)| name);
            if long.len() > 1 && long.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
                return Some(format!("--{long}"));
            }
        }
        let short = token.strip_prefix('-')?;
        (short.len() == 1 && short.chars().all(|c| c.is_ascii_alphanumeric()))
            .then(|| format!("-{short}"))
    }

    fn flags_of(command: &clap::Command) -> BTreeSet<String> {
        let mut flags = BTreeSet::new();
        for arg in command.get_arguments() {
            if let Some(long) = arg.get_long() {
                flags.insert(format!("--{long}"));
            }
            if let Some(short) = arg.get_short() {
                flags.insert(format!("-{short}"));
            }
        }
        flags
    }

    fn known_flags(
        root: &clap::Command,
        sub: &str,
        action: Option<&str>,
    ) -> Option<BTreeSet<String>> {
        if sub == "help" {
            return Some(BTreeSet::new());
        }
        let group = root.get_subcommands().find(|c| c.get_name() == sub)?;
        let target = match action {
            Some(action) => group.get_subcommands().find(|c| c.get_name() == action)?,
            None if group.has_subcommands() => return None,
            None => group,
        };
        let mut flags = flags_of(root);
        flags.extend(flags_of(group));
        flags.extend(flags_of(target));
        flags.insert("--help".to_owned());
        flags.insert("-h".to_owned());
        Some(flags)
    }

    fn documents() -> Vec<(&'static str, &'static str)> {
        let mut docs = vec![("SKILL.md", super::SKILL), ("CORE.md", super::CORE)];
        docs.extend(super::REFERENCES);
        docs
    }

    #[test]
    fn the_skill_names_only_commands_and_flags_the_cli_has() {
        let root = crate::Cli::command();
        let mut failures = Vec::new();
        for (doc, text) in documents() {
            for (sub, action, flags) in mentioned_commands(text) {
                if sub.is_empty() {
                    continue;
                }
                let Some(known) = known_flags(&root, &sub, action.as_deref()) else {
                    failures.push(format!("{doc}: unknown command `acus {sub}`"));
                    continue;
                };
                for flag in flags {
                    if !known.contains(&flag) {
                        failures.push(format!("{doc}: `acus {sub}` has no `{flag}`"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
