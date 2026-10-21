// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::process::Command;

#[cfg(unix)]
use super::harness::pons_command;
use super::harness::{Harness, mock_agent, temp_path};

#[cfg(unix)]
#[tokio::test]
async fn without_an_agent_command_pons_finds_claude_agent_acp_on_path() {
    let dir = temp_path("bin");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(mock_agent(), dir.join("claude-agent-acp")).unwrap();
    let mut command = pons_command(&[]);
    command.env("PATH", &dir);
    let mut h = Harness::spawn(command);
    h.send(json!({"jsonrpc": "2.0", "id": 7, "method": "echo/anything", "params": {"x": 1}}))
        .await;
    let reply = h.recv().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(reply["result"]["echoed"], json!({"x": 1}));
}

/// Returns the default log directory under `home`, as `log::default_dir` picks it.
fn default_log_dir(home: &Path) -> PathBuf {
    match std::env::consts::OS {
        "macos" => home.join("Library/Logs/pons"),
        "windows" => home.join("pons").join("logs"),
        _ => home.join(".local/state/pons"),
    }
}

/// Guards the default path: with no log flags the wire log goes under HOME.
#[tokio::test]
async fn without_log_flags_the_wire_log_goes_under_home() {
    let home = temp_path("home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pons"));
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", &home)
        .env_remove("XDG_STATE_HOME")
        .env_remove("PONS_LOG")
        .arg("--")
        .arg(mock_agent());
    let mut h = Harness::spawn(command);
    h.send(json!({"jsonrpc": "2.0", "id": 1, "method": "echo/anything", "params": {}}))
        .await;
    let _ = h.recv().await;
    let logs = std::fs::read_dir(default_log_dir(&home)).unwrap().count();
    drop(h);
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(logs, 1);
}
