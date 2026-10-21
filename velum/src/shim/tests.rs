// SPDX-License-Identifier: MPL-2.0
use super::*;
use crate::scratch::{Scratch, write_program};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const VELUM: &str = "/opt/velum/bin/velum";

fn path_of<P: AsRef<Path>>(dirs: &[P]) -> OsString {
    std::env::join_paths(dirs.iter().map(|dir| dir.as_ref().as_os_str())).unwrap()
}

#[test]
fn install_checks_refuse_a_claude_or_velum_the_sandbox_could_write() {
    let scratch = Scratch::new("shim-writable");
    let open = scratch.path().join("open");
    let safe = scratch.path().join("safe");
    let shim_dir = scratch.path().join("shim");
    write_program(&open, "claude", "");
    write_program(&safe, "claude", "");
    let settings = serde_json::json!({"filesystem": {"allowWrite": [open], "denyWrite": []}});
    let ok_velum = safe.join("velum");
    let check = |dirs: &[&Path], velum: &Path| {
        ensure_targets_not_writable(&settings, &shim_dir, &path_of(dirs), velum)
    };
    assert!(check(&[&safe], &ok_velum).is_ok());
    let err = check(&[&open, &safe], &ok_velum).unwrap_err();
    assert!(capsa_core::hint::find_hint(&err).is_some());
    assert!(err.to_string().contains("claude"));
    let err = check(&[&safe], &open.join("velum")).unwrap_err();
    assert!(err.to_string().contains("velum"));
}

#[test]
fn quoting_keeps_spaces_and_single_quotes_inside_one_word() {
    assert_eq!(sh_quote(Path::new("/a b/claude")), "'/a b/claude'");
    assert_eq!(sh_quote(Path::new("/it's/claude")), r"'/it'\''s/claude'");
}

#[test]
fn script_quotes_the_real_path_and_carries_the_marker() {
    let text = script(Path::new("/opt/it's here/claude"), Path::new("/o p/velum"));
    assert!(text.lines().any(|line| line == MARKER));
    assert!(text.contains(
        "exec '/o p/velum' run -- '/opt/it'\\''s here/claude' --dangerously-skip-permissions \"$@\"\n"
    ));
}

#[test]
fn baked_velum_round_trips_quoting_and_flags_a_missing_binary() {
    let scratch = Scratch::new("shim-baked");
    let dir = scratch.path().join("shim");
    let odd = scratch.path().join("it's v/velum");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("claude"), script(Path::new("/r/claude"), &odd)).unwrap();
    assert_eq!(baked_velum(&dir.join("claude")), Some(odd.clone()));
    let missing = velum_check(&dir).unwrap();
    assert_eq!(missing.status, Status::Warn);
    assert!(missing.detail.contains("no longer exists"));
    write_program(odd.parent().unwrap(), "velum", "");
    assert_eq!(velum_check(&dir).unwrap().status, Status::Ok);
    // A shim that ran `velum` through PATH is flagged, not trusted.
    std::fs::write(
        dir.join("claude"),
        "#!/bin/sh\n# velum shim\nexec velum run -- '/r/claude' \"$@\"\n",
    )
    .unwrap();
    assert_eq!(velum_check(&dir).unwrap().status, Status::Warn);
    assert!(velum_check(scratch.path()).is_none());
}

#[test]
fn install_bakes_the_absolute_velum_path_and_refuses_a_relative_one() {
    let scratch = Scratch::new("shim-abs-velum");
    let real_dir = scratch.path().join("bin");
    write_program(&real_dir, "claude", "");
    let shim_dir = scratch.path().join("shim");
    let path = path_of(&[&shim_dir, &real_dir]);
    let err = install(&shim_dir, &path, Path::new("velum"), false).unwrap_err();
    assert!(capsa_core::hint::find_hint(&err).is_some());
    assert!(!exists(&shim_dir.join("claude")));
    install(&shim_dir, &path, Path::new(VELUM), false).unwrap();
    assert_eq!(
        baked_velum(&shim_dir.join("claude")),
        Some(PathBuf::from(VELUM))
    );
}

#[test]
fn relative_and_empty_path_entries_never_supply_the_real_claude() {
    let scratch = Scratch::new("shim-rel-path");
    let shim_dir = scratch.path().join("shim");
    let err = install(&shim_dir, OsStr::new(":.:bin"), Path::new(VELUM), false).unwrap_err();
    assert!(err.to_string().contains("no claude on PATH"));
    assert!(capsa_core::hint::find_hint(&err).is_some());
}

#[test]
fn refusals_carry_hints_outside_the_message() {
    let scratch = Scratch::new("shim-hints");
    let shim_dir = scratch.path().join("shim");
    write_program(&shim_dir, "claude", "echo mine");
    let err = uninstall(&shim_dir, false).unwrap_err();
    assert!(!err.to_string().contains("by hand"));
    assert!(capsa_core::hint::find_hint(&err).is_some());
}

#[test]
fn shim_hands_the_real_path_to_velum_intact() {
    let scratch = Scratch::new("shim-roundtrip");
    let real_dir = scratch.path().join("it's a bin");
    write_program(&real_dir, "claude", "");
    let shim_dir = scratch.path().join("shim");
    let velum_dir = scratch.path().join("velum");
    write_program(&velum_dir, "velum", "printf '%s\\n' \"$@\"");
    install(
        &shim_dir,
        &path_of(&[&shim_dir, &real_dir]),
        &velum_dir.join("velum"),
        false,
    )
    .unwrap();
    let output = Command::new("/bin/sh")
        .arg(shim_dir.join("claude"))
        .arg("two words")
        .env("PATH", "")
        .output()
        .unwrap();
    let lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let real_line = real_dir.join("claude").display().to_string();
    assert_eq!(
        lines,
        [
            "run",
            "--",
            real_line.as_str(),
            "--dangerously-skip-permissions",
            "two words"
        ]
    );
}

#[test]
fn install_then_uninstall_writes_and_removes_only_the_shim() {
    let scratch = Scratch::new("shim-install");
    let real_dir = scratch.path().join("bin");
    write_program(&real_dir, "claude", "");
    let shim_dir = scratch.path().join("shim dir");
    let path = path_of(&[&shim_dir, &real_dir]);
    install(&shim_dir, &path, Path::new(VELUM), false).unwrap();
    let target = shim_dir.join("claude");
    assert!(is_shim(&target));
    let mode = std::fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    uninstall(&shim_dir, false).unwrap();
    assert!(!exists(&target));
}

#[test]
fn install_and_uninstall_refuse_a_file_velum_did_not_write() {
    let scratch = Scratch::new("shim-foreign");
    let real_dir = scratch.path().join("bin");
    write_program(&real_dir, "claude", "");
    let shim_dir = scratch.path().join("shim");
    write_program(&shim_dir, "claude", "echo mine");
    let path = path_of(&[&shim_dir, &real_dir]);
    assert!(install(&shim_dir, &path, Path::new(VELUM), false).is_err());
    assert!(uninstall(&shim_dir, false).is_err());
    let kept = std::fs::read_to_string(shim_dir.join("claude")).unwrap();
    assert!(kept.contains("echo mine"));
}

#[test]
fn install_refuses_without_a_real_claude_or_over_another_shim() {
    let scratch = Scratch::new("shim-refuse");
    let shim_dir = scratch.path().join("shim");
    let other = scratch.path().join("other");
    write_program(&other, "claude", "# velum shim");
    assert!(install(&shim_dir, &path_of(&[&shim_dir]), Path::new(VELUM), false).is_err());
    assert!(
        install(
            &shim_dir,
            &path_of(&[&shim_dir, &other]),
            Path::new(VELUM),
            false
        )
        .is_err()
    );
    assert!(!exists(&shim_dir.join("claude")));
}

#[test]
fn dry_run_writes_nothing() {
    let scratch = Scratch::new("shim-dry");
    let real_dir = scratch.path().join("bin");
    write_program(&real_dir, "claude", "");
    let shim_dir = scratch.path().join("shim");
    install(
        &shim_dir,
        &path_of(&[&shim_dir, &real_dir]),
        Path::new(VELUM),
        true,
    )
    .unwrap();
    assert!(!exists(&shim_dir.join("claude")));
}

#[test]
fn position_check_reads_the_order_on_path() {
    let scratch = Scratch::new("shim-position");
    let real_dir = scratch.path().join("bin");
    write_program(&real_dir, "claude", "");
    let shim_dir = scratch.path().join("shim");
    let before = path_of(&[&shim_dir, &real_dir]);
    let after = path_of(&[&real_dir, &shim_dir]);
    let missing = path_of(&[&real_dir]);
    assert_eq!(position_check(&shim_dir, &before).status, Status::Ok);
    assert_eq!(position_check(&shim_dir, &after).status, Status::Warn);
    assert_eq!(position_check(&shim_dir, &missing).status, Status::Warn);
}
