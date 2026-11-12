// SPDX-License-Identifier: MPL-2.0
//! `census self`: the skill, settings.json, the config, the health check, and the update and
//! removal of the binary.

use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::skill::{self, Skill, Target};

/// The skill shipped with this binary, so an installed copy matches its version.
pub(crate) const SKILL: Skill = Skill {
    tool: "census",
    text: include_str!("../skill/SKILL.md"),
    extra: &[],
};

/// This binary as the self-update and uninstall code sees it.
#[cfg(feature = "cmd-update")]
const TOOL: capsa_core::update::Tool = capsa_core::update::Tool {
    name: "census",
    version: env!("CARGO_PKG_VERSION"),
};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Replace this binary with the latest release, then reinstall the skill.
    Update(UpdateArgs),
    /// Prints the skill, or with --install writes it to the skill directory of --target.
    Skill(SkillArgs),
    /// Installs the skill, then adds census's hooks and statusLine to settings.json.
    Setup(crate::setup::Args),
    /// Remove the installed skill, then this binary.
    Uninstall(UninstallArgs),
    /// Shows or sets the limit mode (plan or budget), the monthly budget and the daily limit.
    Config(crate::cmd_config::Args),
    /// Checks the binary, config, skill, hooks, state files and hook log; exits 1 on a failure.
    Doctor(crate::cmd_doctor::Args),
}

#[cfg(feature = "cmd-update")]
use capsa_core::update::{UninstallArgs, UpdateArgs};

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct UpdateArgs {}
#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct UninstallArgs {
    /// Print what would be removed and remove nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(clap::Args)]
struct SkillArgs {
    /// Write SKILL.md to the skill directory of --target instead of printing it.
    #[arg(long)]
    install: bool,
    /// The agent whose skill directory --install writes to: claude, codex or all.
    #[arg(long, value_enum, default_value_t = Target::Claude, requires = "install")]
    target: Target,
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    match a.action {
        Action::Skill(a) => skill(&a),
        Action::Setup(a) => crate::setup::run(&a, out),
        Action::Update(a) => update(a, out.format),
        Action::Uninstall(a) => uninstall(&a, out.format),
        Action::Config(a) => crate::cmd_config::run(a, out),
        Action::Doctor(a) => crate::cmd_doctor::run(out, a.fix),
    }
}

fn skill(a: &SkillArgs) -> Result<Outcome> {
    if !a.install {
        print!("{}", SKILL.text);
        return Ok(Outcome::Found);
    }
    let home = std::env::home_dir().context("no home directory")?;
    for file in skill::install(&home, &SKILL, a.target)? {
        println!("{}", file.display());
    }
    Ok(Outcome::Found)
}

#[cfg(not(feature = "cmd-update"))]
fn update(_: UpdateArgs, _: Format) -> Result<Outcome> {
    Err(capsa_core::hint::update_unavailable(
        crate::setup::TOOL,
        "census",
    ))
}

/// Replaces the binary, then runs the new one's `self skill --install`, since the binary on disk
/// carries the skill of its version.
#[cfg(feature = "cmd-update")]
fn update(a: UpdateArgs, format: Format) -> Result<Outcome> {
    capsa_core::update::run(&TOOL, a, format, |report| {
        capsa_core::update::reinstall_skill(&report.exe, TOOL.name, None, format)
    })
}

#[cfg(not(feature = "cmd-update"))]
fn uninstall(_: &UninstallArgs, _: Format) -> Result<Outcome> {
    Err(capsa_core::hint::uninstall_unavailable(crate::setup::TOOL))
}

/// Removes the binary first and the skill after it, so a refused binary leaves the skill in place
/// and a failed removal can run again. The check comes before both. Under `--json` stdout holds
/// only the removal.
#[cfg(feature = "cmd-update")]
fn uninstall(a: &UninstallArgs, format: Format) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let outcome = capsa_core::update::uninstall_run(&TOOL, a.dry_run, format, |dry| {
        let files = skill::uninstall(&home, &SKILL, dry)?;
        if format != Format::Json {
            let verb = if dry { "would remove" } else { "removed" };
            for file in &files {
                println!("{verb} {}", file.display());
            }
        }
        Ok(())
    })?;
    if format != Format::Json && !a.dry_run {
        let note = capsa_core::update::leftover_note(
            TOOL.name,
            &capsa_core::hooks::Agent::ClaudeCode.path(),
            "hooks and statusLine",
        );
        println!("{note}");
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::SKILL;
    use capsa_core::skill::Target;
    use clap::CommandFactory;
    use std::collections::BTreeMap;
    use std::path::Path;

    /// Returns the value of a frontmatter field.
    fn frontmatter(key: &str) -> Option<&'static str> {
        SKILL
            .text
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
    }

    /// Returns the `census <subcommand> <flags>` calls the skill writes in backticks.
    fn invocations() -> Vec<(String, Vec<String>)> {
        SKILL
            .text
            .split('`')
            .skip(1)
            .step_by(2)
            .filter_map(|span| {
                let mut words = span.split_whitespace();
                if words.next() != Some("census") {
                    return None;
                }
                let cmd = words.next()?.to_owned();
                let flags: Vec<String> = words
                    .filter(|word| word.starts_with("--"))
                    .map(str::to_owned)
                    .collect();
                Some((cmd, flags))
            })
            .collect()
    }

    #[test]
    fn skill_commands_and_flags_exist_in_the_cli() {
        let root = crate::Cli::command();
        let mut commands: BTreeMap<String, Vec<String>> = root
            .get_subcommands()
            .map(|cmd| {
                let flags = cmd
                    .get_arguments()
                    .filter_map(|arg| arg.get_long().map(|long| format!("--{long}")))
                    .collect();
                (cmd.get_name().to_owned(), flags)
            })
            .collect();
        let global: Vec<String> = root
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(|long| format!("--{long}")))
            .collect();
        for flags in commands.values_mut() {
            flags.extend(global.iter().cloned());
            flags.push("--help".to_owned());
        }

        let calls = invocations();
        assert!(!calls.is_empty(), "the skill names no census command");
        for (cmd, flags) in calls {
            let Some(known) = commands.get(&cmd) else {
                panic!("the skill names `census {cmd}`, which the CLI does not define");
            };
            for flag in flags {
                assert!(
                    known.contains(&flag),
                    "the skill names `census {cmd} {flag}`, which the CLI does not define"
                );
            }
        }
    }

    #[test]
    fn the_skill_name_is_the_installed_directory_name() {
        let name = frontmatter("name").expect("the skill has a name");
        assert_eq!(name, SKILL.tool);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "the name must be lowercase a-z, 0-9 and -"
        );
        let dir = Target::Claude
            .dirs(Path::new("/home/test"), SKILL.tool)
            .into_iter()
            .next()
            .expect("the Claude target has a directory");
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some(name));
    }

    #[test]
    fn the_frontmatter_holds_only_a_short_name_and_description() {
        let mut lines = SKILL.text.lines();
        assert_eq!(lines.next(), Some("---"));
        let fields: Vec<(&str, &str)> = lines
            .by_ref()
            .take_while(|line| *line != "---")
            .map(|line| line.split_once(": ").expect("a `key: value` line"))
            .collect();
        let keys: Vec<&str> = fields.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, ["name", "description"]);
        let description = fields[1].1;
        assert!(
            description.chars().count() <= 1024,
            "the description is {} characters, over 1024",
            description.chars().count()
        );
    }
}
