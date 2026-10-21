// SPDX-License-Identifier: MPL-2.0
//! Owns process groups, signals and ownership checks: the only `unsafe` code in velum.
//!
//! The child (srt) runs in its own process group. A SIGHUP, SIGTERM or SIGINT sent to velum
//! is forwarded to that group, velum waits for it, and then still does its clean-up. When
//! the child has exited, processes of its group that survived are killed.

use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

/// The child's process group while it runs, else 0.
static CHILD_PGID: AtomicI32 = AtomicI32::new(0);
/// The last forwarded signal, so one that arrives before the spawn is not lost.
static PENDING: AtomicI32 = AtomicI32::new(0);

/// Records how the child ended.
#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    /// Processes of the child's group were still alive and have been killed.
    pub killed_survivors: bool,
}

/// Returns the effective user id.
pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// Returns whether a process with `pid` exists, including one owned by another user.
pub fn pid_alive(pid: libc::pid_t) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

extern "C" fn forward(sig: libc::c_int) {
    PENDING.store(sig, Ordering::SeqCst);
    let pgid = CHILD_PGID.load(Ordering::SeqCst);
    if pgid > 0 {
        // SAFETY: kill is async-signal-safe.
        unsafe {
            libc::kill(-pgid, sig);
        }
    }
}

/// Makes SIGHUP, SIGTERM and SIGINT forward to the child's group instead of ending velum.
pub fn install_forwarding() {
    for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
        // SAFETY: `act` is fully initialised before the call and `forward` only calls
        // async-signal-safe functions and atomics.
        unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = forward as *const () as libc::sighandler_t;
            libc::sigemptyset(&raw mut act.sa_mask);
            act.sa_flags = libc::SA_RESTART;
            libc::sigaction(sig, &raw const act, std::ptr::null_mut());
        }
    }
}

/// Returns whether stdin is a terminal that velum's own group currently owns.
fn owns_terminal() -> bool {
    // SAFETY: plain system calls on file descriptor 0.
    unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() }
}

/// Makes `pgid` the terminal's foreground group. SIGTTOU is ignored for the call, as a
/// background group would otherwise be stopped by it.
fn set_foreground(pgid: libc::pid_t) {
    // SAFETY: signal and tcsetpgrp are async-signal-safe and have no memory preconditions.
    unsafe {
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::tcsetpgrp(0, pgid);
        libc::signal(libc::SIGTTOU, libc::SIG_DFL);
    }
}

/// Runs in the child between fork and exec.
fn enter_group(tty: bool) {
    // SAFETY: only async-signal-safe functions are called.
    unsafe {
        libc::setpgid(0, 0);
        if tty {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, libc::getpid());
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
        }
    }
}

/// Runs `cmd` in a new process group (the terminal's foreground group, when velum has a
/// terminal), forwards signals to it, waits, and kills what is left of the group.
pub fn run_in_group(cmd: &mut Command) -> io::Result<Finished> {
    let tty = owns_terminal();
    // SAFETY: the closure calls only async-signal-safe functions.
    unsafe {
        cmd.pre_exec(move || {
            enter_group(tty);
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    let pid = libc::pid_t::try_from(child.id()).map_err(io::Error::other)?;
    // SAFETY: setpgid on our own child. It may lose the race with the child's own call.
    unsafe {
        libc::setpgid(pid, pid);
    }
    CHILD_PGID.store(pid, Ordering::SeqCst);
    let pending = PENDING.load(Ordering::SeqCst);
    if pending != 0 {
        // SAFETY: kill with a valid signal number.
        unsafe {
            libc::kill(-pid, pending);
        }
    }
    let status = wait_child(pid, tty);
    CHILD_PGID.store(0, Ordering::SeqCst);
    if tty {
        // SAFETY: getpgrp has no preconditions.
        set_foreground(unsafe { libc::getpgrp() });
    }
    let killed_survivors = kill_survivors(pid);
    Ok(Finished {
        status,
        killed_survivors,
    })
}

/// Waits for `pid`. When the child stops (Ctrl-Z), velum takes the terminal back and stops
/// itself, so the shell sees a stopped job; on resume it hands the terminal over again.
fn wait_child(pid: libc::pid_t, tty: bool) -> ExitStatus {
    loop {
        let mut raw: libc::c_int = 0;
        // SAFETY: `raw` is a valid out pointer for the duration of the call.
        let got = unsafe { libc::waitpid(pid, &raw mut raw, libc::WUNTRACED) };
        if got < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return ExitStatus::from_raw(1 << 8);
        }
        if libc::WIFSTOPPED(raw) {
            suspend(pid, tty);
            continue;
        }
        return ExitStatus::from_raw(raw);
    }
}

fn suspend(pgid: libc::pid_t, tty: bool) {
    if tty {
        // SAFETY: getpgrp has no preconditions.
        set_foreground(unsafe { libc::getpgrp() });
    }
    // SAFETY: stops this process until SIGCONT.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGSTOP);
    }
    if tty {
        set_foreground(pgid);
    }
    // SAFETY: continues the child's group.
    unsafe {
        libc::kill(-pgid, libc::SIGCONT);
    }
}

fn group_alive(pgid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks that the group exists.
    let rc = unsafe { libc::kill(-pgid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Kills what is left of the group of a child that has exited: SIGTERM, then SIGKILL after
/// one second. Returns whether anything was left.
fn kill_survivors(pgid: libc::pid_t) -> bool {
    if !group_alive(pgid) {
        return false;
    }
    for sig in [libc::SIGTERM, libc::SIGKILL] {
        // SAFETY: kill with a valid signal number.
        unsafe {
            libc::kill(-pgid, sig);
        }
        for _ in 0..20 {
            if !group_alive(pgid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    // One test: the signal state is process-wide.
    #[test]
    fn survivors_are_killed_and_signals_reach_the_group() {
        install_forwarding();
        let mut quick = Command::new("sh");
        quick.args(["-c", "sleep 30 & exit 0"]);
        let done = run_in_group(&mut quick).unwrap();
        assert!(done.status.success());
        assert!(done.killed_survivors);

        let mut clean = Command::new("sh");
        clean.args(["-c", "exit 3"]);
        let done = run_in_group(&mut clean).unwrap();
        assert_eq!(done.status.code(), Some(3));
        assert!(!done.killed_survivors);

        let sender = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            // SAFETY: signals this test process, which has the forwarding handler.
            unsafe {
                libc::kill(libc::getpid(), libc::SIGTERM);
            }
        });
        let start = Instant::now();
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);
        let done = run_in_group(&mut slow).unwrap();
        sender.join().unwrap();
        assert!(!done.status.success());
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
