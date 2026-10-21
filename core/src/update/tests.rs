// SPDX-License-Identifier: MPL-2.0

use std::io::Write;

use serde_json::Value;

use super::archive::{extract, verify};
use super::binary::{ensure_named, remove, replace};
use super::release::{archive_name, asset_url, select_release, version};
use super::{Report, State, UpdateError};

fn releases(json: &str) -> Vec<Value> {
    serde_json::from_str(json).unwrap()
}

#[test]
fn compares_versions_numerically() {
    assert!(version("0.10.0") > version("0.9.3"));
    assert!(version("v1.0.0".trim_start_matches('v')) > version("0.99.0"));
    assert_eq!(version("0.5.0"), version("0.5.0\n"));
}

#[test]
fn verifies_checksums() {
    // sha256("abc")
    let sum = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  acus.tar.gz";
    assert!(verify(b"abc", sum).is_ok());
    assert!(matches!(
        verify(b"abd", sum),
        Err(UpdateError::ChecksumMismatch { .. })
    ));
}

#[test]
fn extracts_the_binary_from_both_archive_kinds() {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut h = tar::Header::new_gnu();
    h.set_size(3);
    h.set_mode(0o755);
    h.set_cksum();
    tar.append_data(&mut h, "acus", &b"bin"[..]).unwrap();
    let gz = tar.into_inner().unwrap().finish().unwrap();
    assert_eq!(extract(&gz, "tar.gz", "acus").unwrap(), b"bin");
    assert!(matches!(
        extract(&gz, "tar.gz", "other"),
        Err(UpdateError::MissingInArchive { .. })
    ));

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("acus.exe", opts).unwrap();
    zip.write_all(b"exe").unwrap();
    let data = zip.finish().unwrap().into_inner();
    assert_eq!(extract(&data, "zip", "acus.exe").unwrap(), b"exe");
    assert!(extract(&data, "zip", "other.exe").is_err());
}

#[test]
fn refuses_links_directories_and_empty_files_in_a_tar() {
    let build = |kind: tar::EntryType, body: &[u8]| {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_size(body.len() as u64);
        h.set_mode(0o755);
        if kind == tar::EntryType::Symlink || kind == tar::EntryType::Link {
            h.set_link_name("/bin/sh").unwrap();
        }
        h.set_cksum();
        tar.append_data(&mut h, "acus", body).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    };
    for kind in [
        tar::EntryType::Symlink,
        tar::EntryType::Link,
        tar::EntryType::Directory,
    ] {
        assert!(matches!(
            extract(&build(kind, b""), "tar.gz", "acus"),
            Err(UpdateError::MissingInArchive { .. })
        ));
    }
    assert!(matches!(
        extract(&build(tar::EntryType::Regular, b""), "tar.gz", "acus"),
        Err(UpdateError::MissingInArchive { .. })
    ));
}

#[test]
fn refuses_links_directories_and_empty_files_in_a_zip() {
    let new = || zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let mut link = new();
    link.add_symlink("acus.exe", "/bin/sh", opts).unwrap();
    let mut dir = new();
    dir.add_directory("acus.exe", opts).unwrap();
    let mut empty = new();
    empty.start_file("acus.exe", opts).unwrap();
    for zip in [link, dir, empty] {
        let data = zip.finish().unwrap().into_inner();
        assert!(extract(&data, "zip", "acus.exe").is_err());
    }
}

#[test]
fn replaces_the_binary_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("acus");
    std::fs::write(&exe, "old").unwrap();
    replace(&exe, "acus", b"new").unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(left.len(), 1 + usize::from(cfg!(windows)), "{left:?}");
}

#[cfg(unix)]
#[test]
fn replaced_binary_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("acus");
    std::fs::write(&exe, "old").unwrap();
    replace(&exe, "acus", b"new").unwrap();
    let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111);
}

#[cfg(unix)]
#[test]
fn refuses_to_replace_or_remove_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    let link = dir.path().join("acus");
    std::fs::write(&real, "keep").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(
        replace(&link, "acus", b"new"),
        Err(UpdateError::Symlink { .. })
    ));
    assert!(matches!(remove(&link), Err(UpdateError::Symlink { .. })));
    assert_eq!(std::fs::read(&real).unwrap(), b"keep");
}

#[test]
fn refuses_a_directory_and_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("acus");
    std::fs::create_dir(&sub).unwrap();
    assert!(matches!(
        replace(&sub, "acus", b"new"),
        Err(UpdateError::NotAFile { .. })
    ));
    assert!(matches!(
        replace(&dir.path().join("acus.exe"), "acus", b"new"),
        Err(UpdateError::Write { .. })
    ));
}

#[test]
fn a_planted_temporary_file_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("acus");
    std::fs::write(&exe, "old").unwrap();
    let tmp = dir
        .path()
        .join(format!(".acus.update-{}", std::process::id()));
    std::fs::write(&tmp, "planted").unwrap();
    assert!(replace(&exe, "acus", b"new").is_err());
    assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    assert_eq!(std::fs::read(&tmp).unwrap(), b"planted");
}

#[test]
fn removes_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("acus");
    std::fs::write(&exe, "old").unwrap();
    remove(&exe).unwrap();
    assert!(!exe.exists());
}

#[test]
fn replacing_requires_the_tool_name() {
    let dir = tempfile::tempdir().unwrap();
    let renamed = dir.path().join("renamed");
    std::fs::write(&renamed, "old").unwrap();
    assert!(matches!(
        replace(&renamed, "acus", b"new"),
        Err(UpdateError::NotNamed { .. })
    ));
    assert_eq!(std::fs::read(&renamed).unwrap(), b"old");
}

#[test]
fn removal_requires_the_tool_name() {
    let dir = tempfile::tempdir().unwrap();
    assert!(ensure_named(&dir.path().join("acus"), "acus").is_ok());
    assert!(ensure_named(&dir.path().join("acus.exe"), "acus").is_ok());
    assert!(matches!(
        ensure_named(&dir.path().join("renamed"), "acus"),
        Err(UpdateError::NotNamed { .. })
    ));
}

#[test]
fn selects_the_newest_published_release_of_the_tool() {
    let list = releases(
        r#"[
            {"tag_name": "velum-v0.3.0", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.17.0", "draft": true, "prerelease": false},
            {"tag_name": "acus-v0.16.2-rc.1", "draft": false, "prerelease": true},
            {"tag_name": "acus-v0.16.1", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.16.0", "draft": false, "prerelease": false}
        ]"#,
    );
    assert_eq!(
        select_release(list.clone(), "acus").unwrap().tag,
        "acus-v0.16.1"
    );
    assert_eq!(select_release(list, "velum").unwrap().tag, "velum-v0.3.0");
}

#[test]
fn selects_the_highest_version_whatever_the_listing_order() {
    let list = releases(
        r#"[
            {"tag_name": "acus-v0.16.2", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.9.0", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.17.0", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.18.0", "draft": true, "prerelease": false},
            {"tag_name": "acus-v0.10.0", "draft": false, "prerelease": false}
        ]"#,
    );
    let picked = select_release(list, "acus").unwrap();
    assert_eq!(picked.tag, "acus-v0.17.0");
    // `--force` installs this release, so it must never be below another published one.
    for lower in ["0.16.2", "0.9.0", "0.10.0"] {
        assert!(version(picked.tag.strip_prefix("acus-v").unwrap()) > version(lower));
    }
}

#[test]
fn selects_no_release_without_the_tools_tags() {
    let list = releases(r#"[{"tag_name": "pons-v0.5.0", "draft": false, "prerelease": false}]"#);
    assert!(select_release(list, "acus").is_none());
}

#[test]
fn names_archives_and_finds_their_api_urls() {
    let name = archive_name("acus", "x86_64-pc-windows-msvc", "zip");
    assert_eq!(name, "acus-x86_64-pc-windows-msvc.zip");
    let release: Value = serde_json::from_str(
        r#"{"assets": [
            {"name": "acus-x86_64-pc-windows-msvc.zip", "url": "https://api.github.com/a/7"}
        ]}"#,
    )
    .unwrap();
    assert_eq!(
        asset_url(&release, &name).as_deref(),
        Some("https://api.github.com/a/7")
    );
    assert!(asset_url(&release, "acus-x86_64-pc-windows-msvc.zip.sha256").is_none());
}

#[test]
fn report_names_the_command_to_run() {
    let report = Report {
        tool: "pons",
        current: "0.5.2".into(),
        latest: "0.6.0".into(),
        exe: "/bin/pons".into(),
        state: State::Available,
    };
    assert_eq!(
        report.to_string(),
        "pons 0.6.0 is available (installed 0.5.2); run pons self update"
    );
    assert_eq!(report.outcome(true).code(), 0);
    let current = Report {
        state: State::Current,
        ..report
    };
    assert_eq!(current.outcome(true).code(), 1);
    assert_eq!(current.outcome(false).code(), 0);
}
