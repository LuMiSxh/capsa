// SPDX-License-Identifier: MPL-2.0
//! Writes the launchd property list by hand. The agent runs through zsh, so that `.zshenv` can
//! source the secrets file and the DeepSeek key never enters the plist.

use std::path::{Path, PathBuf};

/// Returns the property list that runs `exe serve` for `label` and logs to `log`.
pub fn render(label: &str, exe: &Path, log: &Path) -> String {
    let command = xml(&format!("exec {} serve", quote(&exe.display().to_string())));
    let log = xml(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/zsh</string>
		<string>-c</string>
		<string>{command}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

/// Quotes `text` as one word for zsh: single quotes, and each `'` written as `'\''`.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Escapes the characters that XML reads as markup in the text of an element.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unxml(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

/// Returns the binary that a property list from `render` runs, or `None` when there is none.
pub fn program_path(text: &str) -> Option<PathBuf> {
    let body = text
        .split_once("<string>exec ")?
        .1
        .split_once("</string>")?
        .0;
    let quoted = body.strip_suffix(" serve")?;
    let inner = quoted.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(PathBuf::from(unxml(inner).replace(r"'\''", "'")))
}

#[cfg(test)]
mod tests {
    use super::{program_path, render};
    use std::path::Path;

    const LABEL: &str = "io.github.lumisxh.vicarius";

    #[test]
    fn plist_runs_the_binary_through_zsh_and_logs_to_the_file() {
        let text = render(
            LABEL,
            Path::new("/opt/vicarius"),
            Path::new("/state/service.log"),
        );
        assert!(
            text.contains(
                "<string>/bin/zsh</string>\n\t\t<string>-c</string>\n\t\t<string>exec '/opt/vicarius' serve</string>"
            ),
            "{text}"
        );
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"), "{text}");
        assert!(text.contains("<key>KeepAlive</key>\n\t<true/>"), "{text}");
        assert_eq!(
            text.matches("<string>/state/service.log</string>").count(),
            2
        );
    }

    #[test]
    fn a_path_with_a_space_and_a_quote_is_quoted_for_zsh() {
        let exe = Path::new("/Users/a b/it's/vicarius");
        let text = render(LABEL, exe, Path::new("/s/service.log"));
        assert!(
            text.contains("<string>exec '/Users/a b/it'\\''s/vicarius' serve</string>"),
            "{text}"
        );
        assert_eq!(program_path(&text).as_deref(), Some(exe));
    }

    #[test]
    fn markup_in_the_paths_is_escaped_and_read_back() {
        let exe = Path::new("/x/a&b<c>\"d\"/vicarius");
        let text = render(LABEL, exe, Path::new("/s/a&b.log"));
        assert!(
            text.contains("/x/a&amp;b&lt;c&gt;&quot;d&quot;/vicarius"),
            "{text}"
        );
        assert!(text.contains("<string>/s/a&amp;b.log</string>"), "{text}");
        assert_eq!(program_path(&text).as_deref(), Some(exe));
    }

    #[test]
    fn a_text_without_a_program_has_no_path() {
        assert_eq!(program_path("<plist></plist>"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_starts_a_binary_whose_path_has_a_space_and_a_quote() {
        use super::quote;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("it's a dir").join("vicarius");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "#!/bin/sh\necho started \"$@\"\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let command = format!("exec {} serve", quote(&exe.display().to_string()));
        let out = std::process::Command::new("/bin/zsh")
            .arg("-c")
            .arg(command)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "started serve");
    }
}
