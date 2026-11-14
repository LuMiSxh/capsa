// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn signal_names_are_conventional() {
    assert_eq!(Signal::Term.name(), "SIGTERM");
    assert_eq!(Signal::Kill.name(), "SIGKILL");
}

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;

    fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while !done() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    #[test]
    fn rejects_group_ids_that_would_reach_everything() {
        for id in [0, 1, u32::MAX] {
            assert!(matches!(
                signal_group(id, Signal::Kill),
                Err(ProcessError::InvalidGroup(got)) if got == id
            ));
            assert!(!group_alive(id));
        }
    }

    #[test]
    fn killing_the_group_ends_the_grandchild() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 60 & echo ready; wait"])
            .stdout(Stdio::piped());
        let mut child = own_group(&mut cmd).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pgid = child.id();
        assert!(group_alive(pgid));

        signal_group(pgid, Signal::Kill).unwrap();
        let status = child.wait().unwrap();

        assert_eq!(exit_code(status), Some(128 + 9));
        assert!(wait_until(Duration::from_secs(5), || !group_alive(pgid)));
        assert!(matches!(
            signal_group(pgid, Signal::Term),
            Err(ProcessError::Signal { .. })
        ));
    }

    #[cfg(feature = "process-async")]
    #[test]
    fn signals_report_the_request_received() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut signals = Signals::new().unwrap();
            // SAFETY: the handler installed above turns SIGHUP into an event instead of exit.
            assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
            assert_eq!(signals.recv().await, Signal::Hup);
        });
    }

    #[cfg(feature = "process-async")]
    #[test]
    fn wait_group_gone_gives_up_while_a_member_lives() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            let mut child = own_group(&mut cmd).spawn().unwrap();
            let pgid = child.id();
            assert!(!wait_group_gone(pgid, Duration::from_millis(60)).await);
            signal_group(pgid, Signal::Kill).unwrap();
            child.wait().unwrap();
            assert!(wait_group_gone(pgid, Duration::from_secs(5)).await);
        });
    }
}

#[cfg(windows)]
#[test]
fn windows_cannot_ask_whether_a_group_lives() {
    assert!(!group_alive(4242));
}
