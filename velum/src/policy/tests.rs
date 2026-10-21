// SPDX-License-Identifier: MPL-2.0

use super::builtin::CWD_DENY_NAMES;
use super::*;

use capsa_core::hint::Hint;
use std::path::Path;

#[test]
fn file_values_extend_defaults_without_duplicates() {
    let text = r#"
[fs]
write = ["/private/tmp", "/opt/cache"]

[env]
pass = ["PATH", "FOO"]

[net]
allow = ["example.com", "claude.ai"]
"#;
    let policy = Policy::from_toml(Path::new("/work"), text).unwrap();
    assert_eq!(
        policy
            .write
            .iter()
            .filter(|p| p.as_str() == "/private/tmp")
            .count(),
        1
    );
    assert_eq!(policy.write.last().map(String::as_str), Some("/opt/cache"));
    assert_eq!(
        policy
            .env_pass
            .iter()
            .filter(|p| p.as_str() == "PATH")
            .count(),
        1
    );
    assert_eq!(policy.env_pass.last().map(String::as_str), Some("FOO"));
    assert_eq!(
        policy
            .net_allow
            .iter()
            .filter(|d| d.as_str() == "claude.ai")
            .count(),
        1
    );
    assert_eq!(
        policy.net_allow.last().map(String::as_str),
        Some("example.com")
    );
}

#[test]
fn missing_policy_file_means_builtin_defaults() {
    let policy = Policy::load(
        Path::new("/nonexistent/velum/policy.toml"),
        Path::new("/work"),
    );
    assert_eq!(policy.unwrap(), Policy::builtin(Path::new("/work")));
}

#[test]
fn default_policy_file_changes_nothing() {
    let policy = Policy::from_toml(Path::new("/work"), DEFAULT_POLICY_TOML).unwrap();
    assert_eq!(policy, Policy::builtin(Path::new("/work")));
}

#[test]
fn builtin_covers_toolchains_and_protects_claude_config() {
    let policy = Policy::builtin(Path::new("/work"));
    assert!(policy.write.iter().any(|p| p == "~/.claude.json"));
    assert!(
        policy
            .write
            .iter()
            .any(|p| p == "~/Library/org.swift.swiftpm")
    );
    assert!(
        policy
            .deny_write
            .iter()
            .any(|p| p == "~/.claude/settings.json")
    );
    assert!(policy.deny_write.iter().any(|p| p == "~/.claude/hooks"));
}

#[test]
fn expands_leading_tilde_only() {
    let expand = |p| expand_entry(p, "/Users/a", Entry::Allow).unwrap();
    assert_eq!(expand("~/.ssh"), "/Users/a/.ssh");
    assert_eq!(expand("~"), "/Users/a");
    assert_eq!(expand("/etc/hosts"), "/etc/hosts");
}

#[test]
fn entries_must_be_absolute_without_parent_dirs_or_tilde_users() {
    let check = |p, kind| expand_entry(p, "/Users/a", kind);
    for bad in ["~other/x", "relative/x", "/a/../b", "~/../x", "", "**/x"] {
        let err = check(bad, Entry::Allow).unwrap_err();
        assert!(matches!(err, PolicyError::InvalidPath { .. }), "{bad}");
        assert!(err.hint().is_some());
    }
    assert!(check("**/.git/hooks", Entry::Deny).is_ok());
    assert!(check("**/../x", Entry::Deny).is_err());
    assert!(check("rel", Entry::Deny).is_err());
    assert!(check("~other", Entry::Deny).is_err());
}

#[test]
fn cwd_must_be_a_project_directory() {
    let home = "/Users/a";
    for bad in [
        "/",
        "/Users",
        "/Users/a",
        "/Users/a/.config/velum",
        "/Users/a/.config/velum/sub",
        "/Users/a/.local/bin",
        "/Users/a/Library/Application Support/velum/run/x",
        "/Users/a/.ssh",
        "/Users/a/.aws/sub",
        "/Users/a/.gnupg",
        "/Users/a/.config",
        "/Users/a/.config/other",
        "/Users/a/.local",
        "/Users/a/.local/share/x",
        "/Users/a/Library",
        "/Users/a/Library/Mobile Documents/x",
        "/Users/a/.claude",
        "/Users/a/.claude/projects",
        "/Users/a/.cargo/registry",
        "/Users/a/.rustup",
    ] {
        let err = check_cwd(Path::new(bad), home).unwrap_err();
        assert!(matches!(err, PolicyError::CwdRefused { .. }), "{bad}");
        assert!(err.hint().is_some());
    }
    for ok in [
        "/Users/a/proj",
        "/Users/a/.configs",
        "/Users/a/Libraryx",
        "/Users/ab",
        "/work",
    ] {
        assert!(check_cwd(Path::new(ok), home).is_ok(), "{ok}");
    }
}

#[test]
fn home_must_be_absolute_and_non_empty() {
    assert!(validate_home("/Users/a").is_ok());
    for bad in ["", "relative", "/a/../b"] {
        let err = validate_home(bad).unwrap_err();
        assert!(matches!(err, PolicyError::BadHome { .. }), "{bad}");
        assert!(err.hint().is_some());
    }
}

#[test]
fn builtin_denies_persistence_paths_but_keeps_cargo_writable() {
    let policy = Policy::builtin(Path::new("/work"));
    assert!(policy.write.iter().any(|p| p == "~/.cargo"));
    for denied in [
        "~/.cargo/bin",
        "~/.cargo/config.toml",
        "~/.cargo/config",
        "~/.claude/settings.local.json",
        "~/.claude/commands",
        "~/.config/velum",
        "~/.local/bin",
        "~/Library/Application Support/velum",
        "/work/.claude",
        "/work/.mcp.json",
        "/work/**/.claude",
        "/work/**/.mcp.json",
        "~/.claude/local",
    ] {
        assert!(policy.deny_write.iter().any(|p| p == denied), "{denied}");
    }
    for name in CWD_DENY_NAMES {
        for denied in [format!("/work/{name}"), format!("/work/**/{name}")] {
            assert!(policy.deny_write.contains(&denied), "{denied}");
        }
    }
    // Git hooks and config are denied per repository, not by relative globs.
    assert!(!policy.deny_write.iter().any(|p| p.contains(".git/")));
}

#[test]
fn env_pass_secret_extends_from_the_file_and_defaults_empty() {
    assert!(
        Policy::builtin(Path::new("/work"))
            .env_pass_secret
            .is_empty()
    );
    let text = "[env]\npass_secret = [\"ANTHROPIC_API_KEY\"]\n";
    let policy = Policy::from_toml(Path::new("/work"), text).unwrap();
    assert_eq!(policy.env_pass_secret, ["ANTHROPIC_API_KEY"]);
}

#[test]
fn builtin_env_pass_has_no_cargo_or_rustup_wildcard() {
    let policy = Policy::builtin(Path::new("/work"));
    assert!(!policy.env_pass.iter().any(|p| p.ends_with('*')));
    for name in ["CARGO_HOME", "CARGO_TARGET_DIR", "RUSTUP_HOME"] {
        assert!(policy.env_pass.iter().any(|p| p == name), "{name}");
    }
}

#[test]
fn env_wildcard_matches_prefix() {
    assert!(env_matches("CARGO_*", "CARGO_HOME"));
    assert!(env_matches("PATH", "PATH"));
    assert!(!env_matches("PATH", "PATHX"));
    assert!(!env_matches("RUSTUP_*", "CARGO_HOME"));
}
