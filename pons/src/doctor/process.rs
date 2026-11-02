// SPDX-License-Identifier: MPL-2.0
//! Lists the running pons processes for `pons self doctor`.

use super::*;

/// A running pons process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub age_secs: u64,
}

/// What the doctor needs from the machine besides the launcher's [`Fs`]. All
/// answers default to "unknown" so tests can do without.
pub trait Sys {
    fn canonicalize(&self, _: &Path) -> Option<PathBuf> {
        None
    }
    fn file_len(&self, _: &Path) -> Option<u64> {
        None
    }
    /// Lists the running pons proxies, not the doctor itself.
    fn pons_processes(&self) -> Vec<Proc> {
        Vec::new()
    }
    /// Returns the seconds since the running pons binary was last modified.
    fn exe_age_secs(&self) -> Option<u64> {
        None
    }
    /// Returns where `manager` puts global binaries.
    fn global_bin_dir(&self, _: pkg::Manager) -> Option<PathBuf> {
        None
    }
}

/// Parses `[[dd-]hh:]mm:ss` (the `etime` of `ps`) into seconds.
#[cfg(any(unix, test))]
pub fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut secs = 0u64;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    for p in parts {
        secs = secs * 60 + p.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

/// Lists the pons proxies in `ps -axo pid=,etime=,command=` output, without `own`.
#[cfg(any(unix, test))]
pub fn parse_ps(text: &str, own: u32) -> Vec<Proc> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(pid), Some(etime), Some(cmd)) = (words.next(), words.next(), words.next()) else {
            continue;
        };
        let (Ok(pid), Some(age_secs)) = (pid.parse::<u32>(), parse_etime(etime)) else {
            continue;
        };
        let doctor = words.any(|w| w == "doctor");
        if pid != own && file_stem(cmd) == "pons" && !doctor {
            out.push(Proc { pid, age_secs });
        }
    }
    out
}

/// Tells whether a pons process started before the installed binary was last changed.
pub fn is_stale(p: &Proc, exe_age: Option<u64>) -> bool {
    exe_age.is_some_and(|age| p.age_secs > age.saturating_add(5))
}
