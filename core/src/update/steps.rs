// SPDX-License-Identifier: MPL-2.0
//! Owns the steps `self update` and `self uninstall` run beside the binary: reinstalling the skill
//! with the new binary, and the note for hooks that uninstall leaves in place.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::format::Format;
use crate::hint::hinted;

/// Runs `<exe> self skill --install`, with `--target <target>` when given, as the reinstall step
/// of `self update`. The new binary runs it because the binary on disk carries the skill of its
/// version. Under JSON its stdout is dropped, so the update still prints one JSON value.
///
/// # Errors
///
/// Fails when the child cannot start or exits non-zero. The hint names the command to run by hand.
pub fn reinstall_skill(
    exe: &Path,
    tool: &str,
    target: Option<&str>,
    format: Format,
) -> anyhow::Result<()> {
    let mut child = Command::new(exe);
    child.args(["self", "skill", "--install"]);
    if let Some(target) = target {
        child.args(["--target", target]);
    }
    if format == Format::Json {
        child.stdout(Stdio::null());
    }
    if child.status().is_ok_and(|status| status.success()) {
        return Ok(());
    }
    Err(hinted(
        "cannot reinstall the skill",
        format!("run {tool} self skill --install"),
    ))
}

/// Returns the note that uninstall leaves the hooks in `settings` for the user to remove.
/// `entries` names what is left, such as `hooks` or `hooks and statusLine`.
pub fn leftover_note(tool: &str, settings: &Path, entries: &str) -> String {
    format!(
        "{entries} in {} are left; remove the {tool} entries by hand",
        settings.display()
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{leftover_note, reinstall_skill};
    use crate::format::Format;
    use crate::hint::find_hint;

    #[cfg(unix)]
    #[test]
    fn reinstall_fails_with_the_manual_command_when_the_child_exits_non_zero() {
        let err =
            reinstall_skill(Path::new("/usr/bin/false"), "acus", None, Format::Agent).unwrap_err();
        assert_eq!(err.to_string(), "cannot reinstall the skill");
        assert_eq!(find_hint(&err), Some("run acus self skill --install"));
    }

    #[cfg(unix)]
    #[test]
    fn reinstall_succeeds_when_the_child_exits_zero() {
        let result = reinstall_skill(
            Path::new("/usr/bin/true"),
            "acus",
            Some("codex"),
            Format::Json,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn leftover_note_names_the_entries_and_the_settings_file() {
        assert_eq!(
            leftover_note(
                "census",
                Path::new("/home/u/.claude/settings.json"),
                "hooks and statusLine"
            ),
            "hooks and statusLine in /home/u/.claude/settings.json are left; \
             remove the census entries by hand"
        );
    }
}
