// SPDX-License-Identifier: MPL-2.0
//! Picks the agent command when none is given, and builds the spawn plan.
//! Program lookup is [`capsa_core::which`]; everything else is a pure function over an
//! injectable [`Env`] and [`Fs`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use capsa_core::which::{self, Env, Fs, Platform};

#[cfg(test)]
mod tests;

/// The adapter package every package-runner fallback starts.
pub const PACKAGE: &str = "@agentclientprotocol/claude-agent-acp";

/// Programs tried in order, with the arguments that start the adapter.
pub const LAUNCHERS: [(&str, &[&str]); 5] = [
    ("claude-agent-acp", &[]),
    ("npx", &["-y", PACKAGE]),
    ("pnpm", &["dlx", PACKAGE]),
    ("pnpx", &[PACKAGE]),
    ("bunx", &[PACKAGE]),
];

/// What to run, and the `PATH` entries it needs when we located it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub program: OsString,
    pub args: Vec<String>,
    /// Directories for the child's `PATH` (program directory first, then the
    /// inherited `PATH`, then the user bin directories). `None` leaves the
    /// environment alone.
    pub path_dirs: Option<Vec<PathBuf>>,
    /// How the command was chosen, for the log.
    pub how: String,
}

/// Picks the first launcher of [`LAUNCHERS`] that can be found.
pub fn detect(env: &Env, fs: &dyn Fs) -> Option<(usize, PathBuf)> {
    LAUNCHERS
        .iter()
        .enumerate()
        .find_map(|(i, (name, _))| which::find_in(env, fs, name, None).map(|p| (i, p)))
}

/// Returns the command to run: `command` when given (resolved through the search
/// dirs if it is a bare name), else the auto-detected launcher.
pub fn plan(env: &Env, fs: &dyn Fs, command: &[String]) -> Result<Spawn, String> {
    let spawn = match command.split_first() {
        Some((program, args)) => explicit(env, fs, program, args),
        None => {
            let (i, path) = detect(env, fs).ok_or_else(|| not_found(env, fs))?;
            let args: Vec<String> = LAUNCHERS[i].1.iter().map(|a| (*a).to_owned()).collect();
            Spawn {
                how: format!("auto-detected {} at {}", LAUNCHERS[i].0, path.display()),
                path_dirs: Some(which::child_path(env, fs, &path)),
                program: path.into_os_string(),
                args,
            }
        }
    };
    if env.platform == Platform::Windows && is_batch(Path::new(&spawn.program)) {
        check_batch_args(&spawn.args)?;
    }
    Ok(spawn)
}

fn explicit(env: &Env, fs: &dyn Fs, program: &str, args: &[String]) -> Spawn {
    let bare = Path::new(program).components().count() == 1 && !Path::new(program).is_absolute();
    let found = bare
        .then(|| which::find_in(env, fs, program, None))
        .flatten();
    match found {
        Some(path) => Spawn {
            how: format!("explicit command, {program} resolved to {}", path.display()),
            path_dirs: Some(which::child_path(env, fs, &path)),
            program: path.into_os_string(),
            args: args.to_vec(),
        },
        None => Spawn {
            how: "explicit command".to_owned(),
            program: program.into(),
            args: args.to_vec(),
            path_dirs: None,
        },
    }
}

fn not_found(env: &Env, fs: &dyn Fs) -> String {
    let names: Vec<&str> = LAUNCHERS.iter().map(|(n, _)| *n).collect();
    let dirs = which::search_dirs(env, fs).len();
    format!(
        "no agent command given and none could be found (looked for {} on PATH and {dirs} directories in total); pass one after `--`, for example: pons -- npx -y {PACKAGE}",
        names.join(", ")
    )
}

/// Tells whether `program` is a `.cmd` or `.bat` file, which Windows can only run
/// through `cmd.exe`.
pub fn is_batch(program: &Path) -> bool {
    program
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| e == "cmd" || e == "bat")
}

/// Checks the arguments of a `.cmd` shim, which runs through
/// `cmd.exe /c`, whose parsing of `"`,
/// `%`, `^`, `&`, `|`, `<`, `>` and `!` can turn data into commands. Rust's
/// `Command` escapes batch arguments itself and refuses what it cannot make
/// safe; this check rejects the same class up front with a clear message,
/// so an argument can never inject a command.
pub fn check_batch_args(args: &[String]) -> Result<(), String> {
    const UNSAFE: &[char] = &[
        '"', '%', '^', '&', '|', '<', '>', '!', '`', '\r', '\n', '\0',
    ];
    match args.iter().find(|a| a.contains(UNSAFE)) {
        Some(bad) => Err(format!(
            "refusing to pass argument {bad:?} to a .cmd/.bat program: it contains a character cmd.exe would interpret (one of \" % ^ & | < > !)"
        )),
        None => Ok(()),
    }
}
