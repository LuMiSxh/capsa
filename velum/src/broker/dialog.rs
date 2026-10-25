// SPDX-License-Identifier: MPL-2.0
//! Asks the person with a macOS dialog. `osascript` runs here, outside the sandbox, and the
//! sandbox cannot send Apple Events, so only the person can answer.

use super::approval::{Approver, Choice, Prompt, Remember};
use super::exec::{Registry, run_capped};
use super::text::clean;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// The dialog gives up, and the request is denied, after this many seconds.
pub const GIVE_UP_SECS: u64 = 120;

const DENY: &str = "Deny";
const ONCE: &str = "Allow once";
const REPO: &str = "Allow for this repo";

/// Shows the dialog through `osascript`.
pub struct Osascript {
    pub give_up_secs: u64,
    pub registry: Arc<Registry>,
}

impl Approver for Osascript {
    fn ask(&self, prompt: &Prompt) -> Choice {
        let buttons = buttons(prompt.remember);
        let list = buttons
            .iter()
            .map(|label| format!("\"{label}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut command = Command::new("/usr/bin/osascript");
        // The text travels as an argument, never inside the script, so it cannot change it.
        command
            .arg("-e")
            .arg("on run argv")
            .arg("-e")
            .arg(format!(
                "display dialog (item 1 of argv) with title \"velum approval\" buttons {{{list}}} \
                 default button \"{DENY}\" cancel button \"{DENY}\" with icon caution \
                 giving up after {}",
                self.give_up_secs
            ))
            .arg("-e")
            .arg("end run")
            .arg(text(prompt));
        let limit = Duration::from_secs(self.give_up_secs + 10);
        match run_capped(command, limit, 1024, &self.registry) {
            Ok(ran) if ran.code == 0 => parse_answer(&ran.output, prompt.remember),
            _ => Choice::Deny,
        }
    }
}

/// Returns the button labels, `Deny` first.
fn buttons(remember: Remember) -> Vec<&'static str> {
    match remember {
        Remember::Never => vec![DENY, ONCE],

        Remember::Repo => vec![DENY, ONCE, REPO],
    }
}

/// Reads `button returned:X, gave up:false` into a choice. A dialog that gave up, a label the
/// prompt did not offer and any other text are all `Deny`.
fn parse_answer(output: &str, remember: Remember) -> Choice {
    let line = output.trim();
    if line.contains("gave up:true") {
        return Choice::Deny;
    }
    let Some(rest) = line.strip_prefix("button returned:") else {
        return Choice::Deny;
    };
    let label = rest.split(", gave up:").next().unwrap_or("");
    let choice = match label {
        ONCE => Choice::Once,
        REPO => Choice::Repo,

        _ => Choice::Deny,
    };
    if choice.offered(remember) {
        choice
    } else {
        Choice::Deny
    }
}

/// Returns the dialog text: the tool, the repository and the labelled details, each value
/// cleaned to one visible line.
pub fn text(prompt: &Prompt) -> String {
    let mut lines = vec![
        format!("The sandboxed agent asks to run: {}", prompt.tool),
        String::new(),
        format!("Repository: {}", clean(&prompt.repo.to_string_lossy())),
    ];
    lines.extend(
        prompt
            .details
            .iter()
            .map(|(label, value)| format!("{label}: {}", clean(value))),
    );
    lines.push(String::new());
    lines.push("It runs outside the sandbox, with your credentials.".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn prompt(remember: Remember, details: Vec<(&'static str, String)>) -> Prompt {
        Prompt {
            tool: "run_outside",
            repo: PathBuf::from("/work/a"),
            details,
            remember,
        }
    }

    #[test]
    fn the_text_shows_agent_data_as_one_clean_line_per_value() {
        let shown = text(&prompt(
            Remember::Never,
            vec![("Command", "echo hi\nrm -rf ~\u{202e}\u{7}".to_string())],
        ));
        assert!(shown.contains("Repository: /work/a"));
        assert!(shown.contains("Command: echo hi\u{21b5}rm -rf ~"));
        assert!(!shown.contains('\u{202e}') && !shown.contains('\u{7}'));
        assert_eq!(shown.matches("Command:").count(), 1);
    }

    #[test]
    fn buttons_follow_what_the_prompt_may_remember() {
        assert_eq!(buttons(Remember::Never), ["Deny", "Allow once"]);
        assert_eq!(
            buttons(Remember::Repo),
            ["Deny", "Allow once", "Allow for this repo"]
        );
    }

    #[test]
    fn answers_map_to_choices_and_anything_else_denies() {
        let answer = |out: &str, remember| parse_answer(out, remember);
        assert_eq!(
            answer(
                "button returned:Allow once, gave up:false\n",
                Remember::Never
            ),
            Choice::Once
        );
        assert_eq!(
            answer(
                "button returned:Allow for this repo, gave up:false",
                Remember::Repo
            ),
            Choice::Repo
        );
        assert_eq!(
            answer(
                "button returned:Allow this session, gave up:false",
                Remember::Repo
            ),
            Choice::Deny
        );
        assert_eq!(
            answer(
                "button returned:Allow for this repo, gave up:false",
                Remember::Never
            ),
            Choice::Deny
        );
        assert_eq!(
            answer("button returned:Allow once, gave up:true", Remember::Never),
            Choice::Deny
        );
        assert_eq!(
            answer("button returned:Deny, gave up:false", Remember::Repo),
            Choice::Deny
        );
        assert_eq!(answer("", Remember::Repo), Choice::Deny);
    }
}
