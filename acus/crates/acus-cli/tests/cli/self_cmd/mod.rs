// SPDX-License-Identifier: MPL-2.0

mod doctor;
mod setup;
mod skill;
mod uninstall;

use crate::common::{acus, acus_env};

#[test]
fn the_old_top_level_names_are_gone() {
    for name in ["skill", "update"] {
        let (code, _, err) = acus(&[name]);
        assert_eq!(code, 2, "{name}: {err}");
        assert!(err.contains("unrecognized subcommand"), "{name}: {err}");
    }
}

#[test]
fn self_lists_update_skill_and_uninstall() {
    let (code, out, _) = acus(&["self", "--help"]);
    assert_eq!(code, 0);
    for name in ["update", "setup", "doctor", "skill", "uninstall"] {
        assert!(out.contains(name), "{name}: {out}");
    }
}

#[test]
fn a_disabled_name_still_blocks_its_self_action() {
    let (code, _, err) = acus_env(&["self", "skill"], &[("ACUS_DISABLE", "skill")]);
    assert_eq!(code, 2);
    assert!(err.contains("disabled"), "{err}");
}

/// Runs acus with HOME, USERPROFILE and CLAUDE_CONFIG_DIR set, so no real user file is touched.
fn sandboxed<'a>(home: &'a str, claude: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("HOME", home),
        ("USERPROFILE", home),
        ("CLAUDE_CONFIG_DIR", claude),
    ]
}

fn json_line(out: &str) -> serde_json::Value {
    assert_eq!(out.lines().count(), 1, "not one JSON line: {out}");
    serde_json::from_str(out.trim()).unwrap()
}
