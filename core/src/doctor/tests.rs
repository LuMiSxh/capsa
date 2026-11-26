// SPDX-License-Identifier: MPL-2.0

use std::fs::{self, File};
use std::path::Path;

use super::log_health::{
    last_ts, log_age_check, log_file_check, log_size_check, now_secs, size_text,
};
use super::{Repaired, check, dir_check, log_checks, ok, path_check, same_file, writable};
use crate::render::{Check, Doctor, Group, Status};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

fn names(checks: &[Check]) -> Vec<&str> {
    checks.iter().map(|c| c.name.as_ref()).collect()
}

#[test]
fn check_and_ok_keep_their_fields() {
    let c = check("binary", Status::Warn, "found", Some("fix it"));
    assert_eq!(c.name, "binary");
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.detail, "found");
    assert_eq!(c.hint.as_deref(), Some("fix it"));

    let c = ok("version", "1.2.3");
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "1.2.3");
    assert_eq!(c.hint, None);
}

#[test]
fn writable_probes_the_directory_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    writable(dir.path()).unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

    // A directory under the probe name makes the probe fail on every platform, even as root.
    let dir = tempfile::tempdir().unwrap();
    let taken = dir
        .path()
        .join(format!(".capsa-doctor-{}", std::process::id()));
    fs::create_dir(taken).unwrap();
    assert!(writable(dir.path()).is_err());
}

#[test]
fn dir_check_reports_a_missing_writable_or_unwritable_directory() {
    let missing = tempfile::tempdir().unwrap();
    let c = dir_check("state dir", &missing.path().join("missing"), "fix it");
    assert_eq!(c.name, "state dir");
    assert_eq!(c.status, Status::Ok);
    assert!(c.detail.ends_with("not created yet"), "{}", c.detail);
    assert_eq!(c.hint, None);

    let writable_dir = tempfile::tempdir().unwrap();
    let c = dir_check("state dir", writable_dir.path(), "fix it");
    assert_eq!(c.status, Status::Ok);
    assert!(c.detail.ends_with("writable"), "{}", c.detail);
    assert_eq!(c.hint, None);

    let taken = tempfile::tempdir().unwrap();
    fs::create_dir(
        taken
            .path()
            .join(format!(".capsa-doctor-{}", std::process::id())),
    )
    .unwrap();
    let c = dir_check("state dir", taken.path(), "fix the permissions");
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("not writable"), "{}", c.detail);
    assert_eq!(c.hint.as_deref(), Some("fix the permissions"));
}

#[test]
fn same_file_compares_spellings_files_missing_paths_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("tool");
    File::create(&file).unwrap();
    assert!(same_file(&file, &dir.path().join(".").join("tool")));

    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    File::create(&a).unwrap();
    File::create(&b).unwrap();
    assert!(!same_file(&a, &b));

    let missing = dir.path().join("missing");
    assert!(same_file(&missing, &missing));
    assert!(!same_file(&missing, &dir.path().join("other")));

    #[cfg(unix)]
    {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(same_file(&file, &link));
    }
}

#[test]
fn path_check_passes_or_warns_about_the_copy_on_path() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("acus");
    File::create(&exe).unwrap();
    let c = path_check("acus", Some(&exe), Some(&exe));
    assert_eq!(c.name, "PATH");
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.hint, None);

    let found = Path::new("/usr/local/bin/acus");
    let c = path_check("acus", Some(found), None);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "/usr/local/bin/acus");

    let copy = dir.path().join("other");
    File::create(&copy).unwrap();
    let c = path_check("acus", Some(&copy), Some(&exe));
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.detail, format!("acus on PATH is {}", copy.display()));
    assert_eq!(
        c.hint.as_deref(),
        Some("the hooks run the acus on PATH; remove the other copy or update it")
    );

    let c = path_check("census", None, None);
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.detail, "census is not on PATH");
    assert_eq!(
        c.hint.as_deref(),
        Some("the hooks run bare `census`; add the directory of this binary to PATH")
    );
}

#[test]
fn log_file_check_reports_a_missing_writable_or_unopenable_log() {
    let dir = tempfile::tempdir().unwrap();
    let c = log_file_check("guard", &dir.path().join("guard.jsonl"));
    assert_eq!(c.name, "guard log file");
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "no log yet");

    let path = dir.path().join("guard.jsonl");
    File::create(&path).unwrap();
    let c = log_file_check("guard", &path);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "writable");

    // A directory cannot be opened for append on any platform, so the check fails as root too.
    let c = log_file_check("guard", dir.path());
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("not writable"), "{}", c.detail);
    assert!(c.hint.is_some());
}

#[test]
fn log_size_check_reports_missing_small_and_oversized_logs() {
    let dir = tempfile::tempdir().unwrap();
    let c = log_size_check("guard", &dir.path().join("missing"), 4 * MIB);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "no log yet");

    let path = dir.path().join("guard.jsonl");
    fs::write(&path, "{}\n").unwrap();
    let c = log_size_check("guard", &path, 4 * MIB);
    assert_eq!(c.name, "guard log size");
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "0 KiB");
    assert_eq!(c.hint, None);

    File::create(&path).unwrap().set_len(4 * MIB + 1).unwrap();
    let c = log_size_check("guard", &path, 4 * MIB);
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.detail, "4096 KiB");
    assert_eq!(
        c.hint.as_deref(),
        Some("the log should rotate at 4 MiB; check the guard runs")
    );
}

#[test]
fn size_text_uses_the_largest_exact_unit() {
    assert_eq!(size_text(4 * MIB), "4 MiB");
    assert_eq!(size_text(1536 * KIB), "1536 KiB");
    assert_eq!(size_text(1000), "1000 B");
    assert_eq!(size_text(0), "0 B");
    assert_eq!(size_text(MIB + 1), "1048577 B");
}

#[test]
fn last_ts_reads_the_last_complete_entry_or_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    fs::write(&path, "{\"ts\":1}\n{\"ts\":7}\n\n\n").unwrap();
    assert_eq!(last_ts(&path), Some(7));

    let path = dir.path().join("long.jsonl");
    let pad = "x".repeat(10 * 1024);
    fs::write(
        &path,
        format!("{{\"ts\":1}}\n{{\"ts\":42,\"pad\":\"{pad}\"}}\n"),
    )
    .unwrap();
    assert_eq!(last_ts(&path), Some(42));

    let path = dir.path().join("torn.jsonl");
    fs::write(&path, "{\"ts\":1}\n{\"ts\":").unwrap();
    assert_eq!(last_ts(&path), None);

    assert_eq!(last_ts(&dir.path().join("missing")), None);
    let path = dir.path().join("empty.jsonl");
    fs::write(&path, "\n\n").unwrap();
    assert_eq!(last_ts(&path), None);
}

#[test]
fn log_age_check_names_the_age_of_the_last_entry_or_its_absence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guard.jsonl");
    fs::write(&path, "{\"ts\":400}\n").unwrap();
    let c = log_age_check("guard", &path, 1000);
    assert_eq!(c.name, "last guard entry");
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "10m ago");

    let c = log_age_check("guard", &dir.path().join("missing"), 1000);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "no entries yet");

    let path = dir.path().join("future.jsonl");
    fs::write(&path, "{\"ts\":2000}\n").unwrap();
    let c = log_age_check("guard", &path, 1000);
    assert_eq!(c.detail, "just now");
}

#[test]
fn log_checks_name_the_log_after_its_label_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let checks = log_checks("guard", &dir.path().join("guard.jsonl"), 4 * MIB);
    assert_eq!(
        names(&checks),
        ["guard log file", "guard log size", "last guard entry"]
    );
    assert!(checks.iter().all(|c| c.status == Status::Ok));
}

#[test]
fn now_secs_is_after_the_start_of_2020() {
    assert!(now_secs() > 1_577_836_800);
}

#[test]
fn repaired_json_keeps_the_report_and_adds_fixed() {
    let report = Doctor::new(
        "tool",
        "1.0.0",
        vec![Group {
            name: "install",
            checks: vec![ok("skill", "installed")],
        }],
    );
    let fixed = vec![ok("hooks", "added 1 hook")];
    let json = serde_json::to_value(Repaired {
        report: &report,
        fixed,
    })
    .unwrap();
    assert_eq!(json["tool"], "tool");
    assert_eq!(json["ok"], true);
    assert_eq!(json["groups"][0]["name"], "install");
    assert_eq!(json["fixed"][0]["name"], "hooks");
}
