// SPDX-License-Identifier: MPL-2.0
//! Owns the built-in defaults every run starts from, and the policy file that
//! `velum self setup` writes.

use super::Policy;
use std::path::Path;

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

/// Holds the policy file written by `velum self setup`. Every entry is commented out, so it
/// changes nothing.
pub const DEFAULT_POLICY_TOML: &str = r#"# velum policy. Layers: built-in defaults, then this file. Entries here extend the defaults.
# Paths may start with ~/. Remove the leading # to enable an entry.

[fs]
# write      = ["~/Documents/projects"]
# deny_read  = ["~/Library/Mail"]
# deny_write = ["~/Documents/projects/shared"]

[env]
# pass = ["CARGO_*"]
# Names containing TOKEN, SECRET, PASSWORD, PASSWD or CREDENTIAL, or ending in _KEY, are
# dropped even when listed in pass. Only this list (exact names, no wildcards) passes them.
# pass_secret = ["ANTHROPIC_API_KEY"]

[net]
# allow = ["github.com"]
"#;

/// Lists names that Claude Code, an editor, git or a shell reads and runs on the next start,
/// which are denied at
/// the top of the working directory and below it.
pub(super) const CWD_DENY_NAMES: &[&str] = &[
    ".claude",
    ".mcp.json",
    ".gitconfig",
    ".vscode",
    ".idea",
    // Zed project settings and tasks run commands.
    ".zed",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".zlogin",
    ".bashrc",
    ".bash_profile",
    ".profile",
];

impl Policy {
    pub fn builtin(cwd: &Path) -> Self {
        let mut write = vec![cwd.display().to_string()];
        write.extend(owned(&[
            "/private/tmp",
            "/private/var/folders",
            "~/.claude",
            "~/.claude.json",
            "~/.cargo",
            "~/Library/Caches",
            "~/Library/Developer",
            "~/Library/org.swift.swiftpm",
        ]));
        // Git hooks and config are denied per repository (`gitguard`), not here.
        let mut deny_write = owned(&[
            "~/.claude/settings.json",
            "~/.claude/settings.local.json",
            "~/.claude/CLAUDE.md",
            "~/.claude/commands",
            "~/.claude/agents",
            "~/.claude/skills",
            "~/.claude/plugins",
            "~/.claude/output-styles",
            "~/.claude/hooks",
            // A local install of claude would otherwise be writable through ~/.claude.
            "~/.claude/local",
            // ~/.cargo stays writable for caches; binaries and config run code.
            "~/.cargo/bin",
            "~/.cargo/config.toml",
            "~/.cargo/config",
            // velum's own files: policy, shim, per-run settings.
            "~/.config/velum",
            "~/.local/bin",
            "~/Library/Application Support/velum",
        ]);
        // Project Claude config runs code on the next start, at the top and in sub-directories.
        // srt reads a relative `**/name` against its own directory, so the globs are anchored.
        for name in CWD_DENY_NAMES {
            deny_write.push(cwd.join(name).display().to_string());
            deny_write.push(format!("{}/**/{name}", cwd.display()));
        }
        Self {
            write,
            deny_read: owned(&["~/.ssh", "~/.aws", "~/.config/secrets.env"]),
            deny_write,
            env_pass: owned(&[
                "PATH",
                "HOME",
                "USER",
                "LANG",
                "TERM",
                "COLORTERM",
                "SHELL",
                "CARGO_HOME",
                "CARGO_TARGET_DIR",
                "CARGO_TERM_COLOR",
                "CARGO_INCREMENTAL",
                "CARGO_BUILD_JOBS",
                "RUSTUP_HOME",
                "RUSTUP_TOOLCHAIN",
            ]),
            env_pass_secret: Vec::new(),
            net_allow: owned(&[
                "api.anthropic.com",
                "*.anthropic.com",
                "claude.ai",
                "platform.claude.com",
            ]),
        }
    }
}
