// SPDX-License-Identifier: MPL-2.0
//! Process groups: starting a child in its own, signalling it and checking it.

use std::process::{Command, ExitStatus};

use super::ProcessError;

/// A request sent to a process group. Windows maps every variant to the same tree kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Asks the group to end (SIGTERM).
    Term,
    /// Reports a closed terminal (SIGHUP).
    Hup,
    /// Reports an interrupt (SIGINT).
    Int,
    /// Ends the group without a chance to clean up (SIGKILL).
    Kill,
}

impl Signal {
    /// Returns the conventional name, as in `SIGTERM`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Term => "SIGTERM",
            Self::Hup => "SIGHUP",
            Self::Int => "SIGINT",
            Self::Kill => "SIGKILL",
        }
    }

    #[cfg(unix)]
    fn number(self) -> i32 {
        match self {
            Self::Term => libc::SIGTERM,
            Self::Hup => libc::SIGHUP,
            Self::Int => libc::SIGINT,
            Self::Kill => libc::SIGKILL,
        }
    }
}

/// Makes the child lead a new process group, so its pid is the group id for [`signal_group`].
/// Does nothing on Windows. For a tokio command, pass `Command::as_std_mut()`.
pub fn own_group(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Sends `sig` to the process group `pgid`, which is the pid of a child started with
/// [`own_group`]. Call it while the child is unreaped, or right after reaping it to sweep what
/// it left behind: a group id stays taken while any member lives.
///
/// # Errors
///
/// [`ProcessError::InvalidGroup`] for 0, 1 or an id beyond the pid range;
/// [`ProcessError::Signal`] when the group does not exist or the system refuses.
pub fn signal_group(pgid: u32, sig: Signal) -> Result<(), ProcessError> {
    #[cfg(unix)]
    {
        let id = valid_group(pgid)?;
        // SAFETY: killpg(2) takes plain integers and touches no memory of ours; `id` is above 1,
        // so only the process group of the caller's own child is addressed.
        let rc = unsafe { libc::killpg(id, sig.number()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(ProcessError::Signal {
                pgid,
                source: std::io::Error::last_os_error(),
            })
        }
    }
    #[cfg(windows)]
    {
        use std::process::Stdio;
        let _ = sig;
        let status = Command::new("taskkill")
            .args(["/PID", &pgid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|source| ProcessError::Signal { pgid, source })?;
        if status.success() {
            Ok(())
        } else {
            Err(ProcessError::Signal {
                pgid,
                source: std::io::Error::other("taskkill failed"),
            })
        }
    }
}

/// Returns whether any member of the process group `pgid` is still running. Always false on
/// Windows, which cannot ask.
#[must_use]
pub fn group_alive(pgid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(id) = valid_group(pgid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the group exists.
        let rc = unsafe { libc::killpg(id, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        false
    }
}

/// Returns the shell-style exit code: the code itself, or 128 plus the signal that ended the
/// process on Unix. `None` when the status has neither.
#[must_use]
pub fn exit_code(status: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.code().or_else(|| status.signal().map(|s| 128 + s))
    }
    #[cfg(not(unix))]
    {
        status.code()
    }
}

#[cfg(unix)]
fn valid_group(pgid: u32) -> Result<i32, ProcessError> {
    match i32::try_from(pgid) {
        Ok(id) if id > 1 => Ok(id),
        _ => Err(ProcessError::InvalidGroup(pgid)),
    }
}
