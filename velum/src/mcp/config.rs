// SPDX-License-Identifier: MPL-2.0
//! Owns the MCP config that makes the approval broker's tools available to a claude run: a file
//! in the run directory that names this velum binary as the server, passed with `--mcp-config`.

use crate::rundir::RunDir;
use crate::writable;
use anyhow::{Context, Result, ensure};
use capsa_core::hint::ResultExt;
use capsa_core::which;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The file in the run directory that claude reads.
const FILE: &str = "mcp.json";

/// Returns the argv to run. When the broker started and `cmd` starts claude, writes `mcp.json`
/// into `run_dir` and puts `--mcp-config=<file>` right after the program. Any other command
/// comes back unchanged, and no file is written. The single token matters: `--mcp-config` takes
/// a variadic list, so a separate path would swallow the user's arguments as more config files.
pub fn with_broker_config(
    cmd: &[String],
    broker: bool,
    path: &OsStr,
    run_dir: &RunDir,
    settings: &Value,
) -> Result<Vec<String>> {
    let Some((program, args)) = cmd.split_first() else {
        return Ok(Vec::new());
    };
    if !broker || !starts_claude(cmd, path) {
        return Ok(cmd.to_vec());
    }
    let velum = std::env::current_exe().context("finding the velum binary")?;
    ensure!(
        velum.is_absolute(),
        "the velum binary path {} is not absolute",
        velum.display()
    );
    let body = json!({"mcpServers": {"velum": {
        "command": velum.to_string_lossy(),
        "args": ["mcp"],
    }}});
    let file = run_dir.write_file(FILE, body.to_string().as_bytes(), 0o600)?;
    // The sandbox reads the file (it is outside `denyRead`), and must not be able to replace it
    // before claude starts.
    writable::ensure_not_writable(settings, "MCP config", &file).hinted()?;
    let mut argv = vec![program.clone(), format!("--mcp-config={}", file.display())];
    argv.extend_from_slice(args);
    Ok(argv)
}

/// Returns whether `cmd` starts claude: its program is named `claude`, or the file it resolves to
/// (through symlinks, on `path` for a bare name) is.
fn starts_claude(cmd: &[String], path: &OsStr) -> bool {
    let Some(program) = cmd.first() else {
        return false;
    };
    let named_claude = |file: &Path| file.file_name() == Some(OsStr::new("claude"));
    if named_claude(Path::new(program)) {
        return true;
    }
    resolve(program, path)
        .and_then(|found| std::fs::canonicalize(found).ok())
        .is_some_and(|real| named_claude(&real))
}

/// Returns the file `program` runs: the path itself, or the first executable match on `path`.
fn resolve(program: &str, path: &OsStr) -> Option<PathBuf> {
    if Path::new(program).components().count() > 1 {
        return Some(PathBuf::from(program));
    }
    which::find_on_path(program, path, None).map(|(_, found)| found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Policy;
    use crate::scratch::{Scratch, write_program};
    use crate::srt;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn settings(home: &str) -> Value {
        srt::settings(&Policy::builtin(Path::new("/work")), home).unwrap()
    }

    #[test]
    fn the_config_goes_right_after_the_program_and_keeps_the_users_args() {
        let scratch = Scratch::new("mcp-argv");
        let home = scratch.path().to_str().unwrap();
        let run_dir = RunDir::create(home).unwrap();
        let cmd = argv(&["claude", "-p", "hi", "--strict-mcp-config"]);
        let out =
            with_broker_config(&cmd, true, OsStr::new(""), &run_dir, &settings(home)).unwrap();
        let file = run_dir.child(FILE).unwrap();
        assert_eq!(
            out,
            [
                "claude".to_string(),
                format!("--mcp-config={}", file.display()),
                "-p".to_string(),
                "hi".to_string(),
                "--strict-mcp-config".to_string(),
            ]
        );
    }

    #[test]
    fn claude_is_named_by_its_basename_or_by_the_file_it_resolves_to() {
        let scratch = Scratch::new("mcp-claude-name");
        let bin = scratch.path().join("bin");
        write_program(&bin, "claude", "echo 2.1.163");
        write_program(&bin, "claude-agent-acp", "echo acp");
        std::os::unix::fs::symlink(bin.join("claude"), bin.join("cc")).unwrap();
        let path = bin.as_os_str();
        assert!(starts_claude(&argv(&["claude"]), path));
        assert!(starts_claude(
            &argv(&["/opt/homebrew/bin/claude"]),
            OsStr::new("")
        ));
        assert!(starts_claude(&argv(&["cc"]), path));
        assert!(!starts_claude(&argv(&["claude-agent-acp"]), path));
        assert!(!starts_claude(&argv(&["/path/to/claude-agent-acp"]), path));
        assert!(!starts_claude(&argv(&["sh", "-c", "claude"]), path));
    }

    #[test]
    fn no_config_is_written_without_a_broker_or_for_other_programs() {
        let scratch = Scratch::new("mcp-no-injection");
        let home = scratch.path().to_str().unwrap();
        let run_dir = RunDir::create(home).unwrap();
        let settings = settings(home);
        for (cmd, broker) in [
            (argv(&["claude", "-p"]), false),
            (argv(&["claude-agent-acp"]), true),
            (argv(&["sh", "-c", "true"]), true),
        ] {
            let out =
                with_broker_config(&cmd, broker, OsStr::new(""), &run_dir, &settings).unwrap();
            assert_eq!(out, cmd);
            assert!(!run_dir.child(FILE).unwrap().exists());
        }
    }

    #[test]
    fn the_config_names_the_running_velum_by_absolute_path() {
        let scratch = Scratch::new("mcp-json");
        let home = scratch.path().to_str().unwrap();
        let run_dir = RunDir::create(home).unwrap();
        with_broker_config(
            &argv(&["claude"]),
            true,
            OsStr::new(""),
            &run_dir,
            &settings(home),
        )
        .unwrap();
        let text = std::fs::read_to_string(run_dir.child(FILE).unwrap()).unwrap();
        let velum = std::env::current_exe().unwrap();
        let expected = json!({"mcpServers": {"velum": {
            "command": velum.to_string_lossy(),
            "args": ["mcp"],
        }}});
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), expected);
        assert!(velum.is_absolute());
    }

    #[test]
    fn the_config_file_is_not_writable_by_the_sandbox() {
        let scratch = Scratch::new("mcp-not-writable");
        let home = scratch.path().to_str().unwrap();
        let run_dir = RunDir::create(home).unwrap();
        let settings = settings(home);
        with_broker_config(
            &argv(&["claude"]),
            true,
            OsStr::new(""),
            &run_dir,
            &settings,
        )
        .unwrap();
        let file = run_dir.child(FILE).unwrap();
        assert_eq!(writable::writable_by(&settings, &file), None);
    }

    #[test]
    fn injection_refuses_when_the_sandbox_could_write_the_config() {
        let scratch = Scratch::new("mcp-writable");
        let home = scratch.path().to_str().unwrap();
        let run_dir = RunDir::create(home).unwrap();
        let mut settings = settings(home);
        settings["filesystem"]["denyWrite"] = json!([]);
        settings["filesystem"]["allowWrite"] = json!([run_dir.path().to_string_lossy()]);
        let result = with_broker_config(
            &argv(&["claude"]),
            true,
            OsStr::new(""),
            &run_dir,
            &settings,
        );
        assert!(result.is_err());
    }
}
