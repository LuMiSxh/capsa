// SPDX-License-Identifier: MPL-2.0

use std::io::Write;

use serde_json::Value;

use super::archive::{extract, verify};
use super::binary::{ensure_named, remove, replace};
use super::release::{
    archive_name, asset_url, authorization, listing_error, resolve_token, select_release, version,
};
use super::{Removal, Report, State, Tool, UpdateError, withdraw_bulletins};
use crate::format::Format;
use crate::hint::Hint;
use crate::render::emit_to;

fn releases(json: &str) -> Vec<Value> {
    serde_json::from_str(json).unwrap()
}

#[test]
fn withdrawing_removes_the_tools_bulletins_and_leaves_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let tool = Tool {
        name: "acus",
        version: "0.2.0",
    };
    for name in ["acus.json", "acus-release.json", "census.json"] {
        std::fs::write(dir.path().join(name), "{}").unwrap();
    }
    withdraw_bulletins(dir.path(), &tool).unwrap();
    assert!(!dir.path().join("acus.json").exists());
    assert!(!dir.path().join("acus-release.json").exists());
    assert!(dir.path().join("census.json").exists());
    withdraw_bulletins(dir.path(), &tool).unwrap();
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
fn extracts_the_binary_and_refuses_non_regular_entries_in_both_archive_kinds() {
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
fn replaces_the_binary_in_place_and_makes_it_executable() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("acus");
    std::fs::write(&exe, "old").unwrap();
    replace(&exe, "acus", b"new").unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(left.len(), 1 + usize::from(cfg!(windows)), "{left:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }
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
fn replace_refuses_a_directory_a_missing_file_and_a_planted_temporary_file() {
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
fn replacing_or_removing_requires_the_tool_name() {
    let dir = tempfile::tempdir().unwrap();
    let renamed = dir.path().join("renamed");
    std::fs::write(&renamed, "old").unwrap();
    assert!(matches!(
        replace(&renamed, "acus", b"new"),
        Err(UpdateError::NotNamed { .. })
    ));
    assert_eq!(std::fs::read(&renamed).unwrap(), b"old");
    assert!(ensure_named(&dir.path().join("acus"), "acus").is_ok());
    assert!(ensure_named(&dir.path().join("acus.exe"), "acus").is_ok());
    assert!(matches!(
        ensure_named(&dir.path().join("renamed"), "acus"),
        Err(UpdateError::NotNamed { .. })
    ));
}

#[test]
fn select_release_picks_the_newest_published_release_whatever_the_listing_order() {
    let list = releases(
        r#"[
            {"tag_name": "census-v0.3.0", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.17.0", "draft": true, "prerelease": false},
            {"tag_name": "acus-v0.16.2-rc.1", "draft": false, "prerelease": true},
            {"tag_name": "acus-v0.16.1", "draft": false, "prerelease": false},
            {"tag_name": "acus-v0.16.0", "draft": false, "prerelease": false}
        ]"#,
    );
    for (tool, expected) in [("acus", "acus-v0.16.1"), ("census", "census-v0.3.0")] {
        assert_eq!(
            select_release(list.clone(), tool).unwrap().tag,
            expected,
            "{tool}"
        );
    }
    let other = releases(r#"[{"tag_name": "pons-v0.5.0", "draft": false, "prerelease": false}]"#);
    assert!(select_release(other, "acus").is_none());

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

fn pons_report(state: State) -> Report {
    Report {
        tool: "pons",
        current: "0.5.2".into(),
        latest: "0.6.0".into(),
        exe: "/bin/pons".into(),
        state,
    }
}

#[test]
fn report_names_the_command_and_renders_its_display_line_in_every_format() {
    let named = pons_report(State::Available);
    assert_eq!(
        named.to_string(),
        "pons 0.6.0 is available (installed 0.5.2); run pons self update"
    );
    assert_eq!(named.outcome(true).code(), 0);
    let current = Report {
        state: State::Current,
        ..named
    };
    assert_eq!(current.outcome(true).code(), 1);
    assert_eq!(current.outcome(false).code(), 0);

    let report = pons_report(State::Available);
    let mut out = Vec::new();
    emit_to(&report, Format::Agent, false, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("{report}\n"));

    assert_eq!(
        serde_json::to_value(pons_report(State::Updated)).unwrap(),
        serde_json::json!({
            "tool": "pons",
            "current": "0.5.2",
            "latest": "0.6.0",
            "exe": "/bin/pons",
            "state": "updated",
        })
    );

    let mut out = Vec::new();
    emit_to(&report, Format::Human, false, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("pons self update\n"), "{text}");
    assert!(text.contains("installed"), "{text}");
    assert!(text.contains("run pons self update"), "{text}");
}

#[test]
fn removal_agent_text_and_json_keep_the_display_line() {
    let removal = Removal {
        path: "/bin/pons".into(),
        removed: false,
    };
    let mut out = Vec::new();
    emit_to(&removal, Format::Agent, false, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("{removal}\n"));
    assert_eq!(
        serde_json::to_value(&removal).unwrap(),
        serde_json::json!({"path": "/bin/pons", "removed": false})
    );
}

fn status_error(code: u16) -> UpdateError {
    UpdateError::Http {
        url: "https://api.github.com/repos/LuMiSxh/capsa/releases".to_owned(),
        source: Box::new(ureq::Error::StatusCode(code)),
    }
}

#[test]
fn token_resolution_and_the_authorization_header() {
    let cases: &[(Option<&str>, Option<&str>, Option<&str>)] = &[
        (Some(" abc \n"), Some("gh-token"), Some("abc")),
        (Some("  "), Some("gh-token"), Some("gh-token")),
        (None, Some("gh-token"), Some("gh-token")),
        (None, None, None),
    ];
    for (env, gh, expected) in cases {
        let token = resolve_token(env.map(str::to_owned), || gh.map(str::to_owned));
        assert_eq!(token.as_deref(), *expected, "{env:?} {gh:?}");
    }
    assert_eq!(authorization(None), None);
    assert_eq!(authorization(Some("abc")).as_deref(), Some("Bearer abc"));
}

#[test]
fn http_errors_hint_at_the_token_fix_and_name_a_missing_release() {
    let rate_limit =
        Some("GitHub may rate-limit anonymous requests: set GITHUB_TOKEN or run `gh auth login`");
    let rejected = Some("GitHub rejected the token: check GITHUB_TOKEN, or run `gh auth login`");
    let cases = [
        (403, rate_limit),
        (429, rate_limit),
        (401, rejected),
        (500, None),
    ];
    for (code, expected) in cases {
        assert_eq!(
            status_error(code).hint().as_deref(),
            expected,
            "status {code}"
        );
    }

    let acus = Tool {
        name: "acus",
        version: "0.1.0",
    };
    assert!(matches!(
        listing_error(status_error(404), &acus),
        UpdateError::NoRelease { tool: "acus" }
    ));
    assert!(matches!(
        listing_error(status_error(500), &acus),
        UpdateError::Http { .. }
    ));
}
