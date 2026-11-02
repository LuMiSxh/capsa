// SPDX-License-Identifier: MPL-2.0

use tokio::process::Command;

use super::harness::{output_retrying_busy, temp_path};

#[tokio::test]
async fn doctor_prints_a_report_and_exits_zero() {
    let log = temp_path("doctor.log");
    std::fs::write(
        &log,
        "{\"ts\":1,\"level\":\"info\",\"event\":\"note\",\"ms\":0,\"dir\":\"pons\",\"text\":\"cancel filter fired: dropped session/cancel for s1\"}\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pons"))
        .args([
            "self",
            "doctor",
            "--log",
            log.to_str().unwrap(),
            "--agent-cmd",
            "my-agent",
            "-x",
        ])
        .output()
        .await
        .unwrap();
    let _ = std::fs::remove_file(&log);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.is_ascii(), "{text}");
    assert!(
        text.contains("pons self doctor") && text.contains("launcher"),
        "{text}"
    );
    assert!(text.contains("my-agent -x"), "{text}");
    assert!(text.contains("1 dropped"), "{text}");
}

#[tokio::test]
async fn help_mentions_the_auto_detected_launcher() {
    let out = Command::new(env!("CARGO_BIN_EXE_pons"))
        .arg("--help")
        .output()
        .await
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.is_ascii(), "{text}");
    assert!(
        text.contains("claude-agent-acp") && text.contains("bunx"),
        "{text}"
    );
    assert!(text.contains("--force-features"), "{text}");
}

#[tokio::test]
async fn self_lists_its_actions_and_doctor_is_no_longer_top_level() {
    let pons = || Command::new(env!("CARGO_BIN_EXE_pons"));
    let out = pons().args(["self", "--help"]).output().await.unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for action in ["doctor", "setup", "update", "uninstall"] {
        assert!(text.contains(action), "{action}: {text}");
    }
    let out = pons().arg("doctor").output().await.unwrap();
    assert!(!out.status.success());
}

#[tokio::test]
async fn uninstall_dry_run_names_the_binary_and_keeps_it() {
    let out = Command::new(env!("CARGO_BIN_EXE_pons"))
        .args(["self", "uninstall", "--dry-run"])
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("would remove "), "{text}");
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_pons")).is_file());
}

#[tokio::test]
async fn uninstall_removes_a_copy_of_the_binary() {
    let dir = temp_path("uninstall");
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("pons");
    std::fs::copy(env!("CARGO_BIN_EXE_pons"), &copy).unwrap();
    let mut command = Command::new(&copy);
    command.args(["self", "uninstall"]);
    let out = output_retrying_busy(&mut command).await.unwrap();
    let gone = !copy.exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success());
    assert!(gone);
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_pons")).is_file());
}
