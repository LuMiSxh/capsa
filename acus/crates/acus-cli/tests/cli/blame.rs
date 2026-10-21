// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::Path;

#[test]
fn blame_groups_lines_by_commit() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let (first, second) = ("2024-01-10T12:00:00+0000", "2024-02-01T12:00:00+0000");
    git_at(p, first, &["init", "-q"]);
    std::fs::write(
        p.join("a.rs"),
        "fn one() {\n    1\n}\n\nfn two() {\n    2\n}\n",
    )
    .unwrap();
    git_at(p, first, &["add", "."]);
    git_at(p, first, &["commit", "-qm", "first"]);
    std::fs::write(
        p.join("a.rs"),
        "fn one() {\n    1\n}\n\nfn two() {\n    22\n}\n",
    )
    .unwrap();
    git_at(p, second, &["commit", "-qam", "second"]);
    let rev = |r: &str| git_at(p, first, &["rev-parse", r]).trim().to_string();
    let (full_first, full_second) = (rev("HEAD~1"), rev("HEAD"));

    let (code, out, err) = acus_in(p, &["blame", "a.rs#two"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        format!(
            "5-5  {}  t  2024-01-10  first\n6-6  {}  t  2024-02-01  second\n7-7  {}  t  2024-01-10  first\n",
            &full_first[..8],
            &full_second[..8],
            &full_first[..8]
        )
    );

    let (code, out, _) = acus_in(p, &["blame", "a.rs:6", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["commit"].as_str(), Some(full_second.as_str()));
    assert_eq!(
        (v[0]["start"].as_u64(), v[0]["end"].as_u64()),
        (Some(6), Some(6))
    );

    let (code, out, err) = acus_in(p, &["blame", "nosuch.rs"], "");
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (2, "", "error: path not found: nosuch.rs\n")
    );
}

#[test]
fn blame_works_from_any_directory_and_names_uncommitted_lines() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let date = "2024-01-10T12:00:00+0000";
    git_at(p, date, &["init", "-q"]);
    std::fs::write(p.join("a.rs"), "fn one() {\n    1\n}\n").unwrap();
    git_at(p, date, &["add", "."]);
    git_at(p, date, &["commit", "-qm", "first"]);
    std::fs::write(p.join("a.rs"), "fn one() {\n    2\n}\n").unwrap();

    // From the filesystem root the absolute path still blames the file in its repository.
    let abs = p.join("a.rs");
    let abs = abs.to_str().unwrap();
    let (code, out, err) = acus_in(Path::new("/"), &["blame", &format!("{abs}:1-2")], "");
    assert_eq!((code, err.as_str()), (0, ""));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("1-1  ") && lines[0].ends_with("  t  2024-01-10  first"));
    assert!(lines[1].starts_with("2-2  00000000  -  ") && lines[1].ends_with("  uncommitted"));

    let (code, out, _) = acus_in(
        Path::new("/"),
        &["blame", &format!("{abs}:2"), "--json"],
        "",
    );
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v[0]["commit"].as_str(), Some("0".repeat(40).as_str()));
    assert_eq!(v[0]["author"].as_str(), Some("-"));
    assert_eq!(v[0]["summary"].as_str(), Some("uncommitted"));
}
