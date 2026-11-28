// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::process::Command;

use super::harness::{Harness, mock_agent, pons_command, temp_path};

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

/// Sends `mock/env` through pons and returns what the mock agent received.
async fn agent_env(flags: &[&str], inherited_model: Option<&str>) -> serde_json::Value {
    let mut command = pons_command(flags);
    command
        .env_remove("ANTHROPIC_MODEL")
        .env_remove("ANTHROPIC_CUSTOM_MODEL_OPTION")
        .env_remove("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME")
        .arg("--")
        .arg(mock_agent());
    if let Some(model) = inherited_model {
        command.env("ANTHROPIC_MODEL", model);
    }
    let mut h = Harness::spawn(command);
    h.send(json!({"jsonrpc": "2.0", "id": 1, "method": "mock/env"}))
        .await;
    h.recv().await["result"]["env"].clone()
}

#[tokio::test]
async fn without_model_flags_the_agent_environment_is_unchanged() {
    let env = agent_env(&[], Some("claude-sonnet-4-5")).await;
    assert_eq!(
        env,
        json!({
            "ANTHROPIC_MODEL": "claude-sonnet-4-5",
            "ANTHROPIC_CUSTOM_MODEL_OPTION": null,
            "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": null,
        })
    );
}

#[tokio::test]
async fn model_flags_reach_the_agent_and_override_an_inherited_model() {
    let env = agent_env(
        &[
            "--model",
            "deepseek-flash",
            "--model-name",
            "DeepSeek Flash",
        ],
        Some("inherited-model"),
    )
    .await;
    assert_eq!(
        env,
        json!({
            "ANTHROPIC_MODEL": "deepseek-flash",
            "ANTHROPIC_CUSTOM_MODEL_OPTION": "deepseek-flash",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": "DeepSeek Flash",
        })
    );
}

#[tokio::test]
async fn model_option_adds_only_the_picker_row() {
    let env = agent_env(&["--model-option", "deepseek-v4-pro"], None).await;
    assert_eq!(
        env,
        json!({
            "ANTHROPIC_MODEL": null,
            "ANTHROPIC_CUSTOM_MODEL_OPTION": "deepseek-v4-pro",
            "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME": null,
        })
    );
}

#[tokio::test]
async fn an_agent_that_cannot_start_is_reported_with_the_doctor_hint() {
    let mut command = super::harness::pons_command(&[]);
    command.arg("--").arg(temp_path("no-such-agent"));
    let out = command.output().await.unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("failed to spawn agent"), "{err}");
    assert!(err.contains("pons self doctor"), "{err}");
}

/// Returns the default log directory under `home`, pons's state directory.
fn default_log_dir(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join("pons").join("state")
    } else {
        home.join(".local/state/pons")
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
