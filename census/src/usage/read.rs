// SPDX-License-Identifier: MPL-2.0
//! Owns reading transcripts: parsing runs on every core, and the stats come back in job order.

use super::{FileStats, claude, codex, session_id};
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

/// One transcript to read: its agent, the root it was found under and its path.
pub struct Job {
    pub agent: &'static str,
    pub root: PathBuf,
    pub path: PathBuf,
}

/// Reads the jobs not older than `cutoff`, split over the available threads, in job order.
pub fn read_all(jobs: &[Job], cutoff: Option<SystemTime>) -> Vec<FileStats> {
    let threads = std::thread::available_parallelism().map_or(1, NonZero::get);
    let next = AtomicUsize::new(0);
    let mut read: Vec<(usize, FileStats)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(i) else {
                            break;
                        };
                        if let Some(st) = read_one(job, cutoff) {
                            done.push((i, st));
                        }
                    }
                    done
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| {
                w.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    read.sort_by_key(|(i, _)| *i);
    read.into_iter().map(|(_, st)| st).collect()
}

fn read_one(job: &Job, cutoff: Option<SystemTime>) -> Option<FileStats> {
    let p = &job.path;
    if cutoff.is_some_and(|c| p.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < c)) {
        return None;
    }
    // ponytail: unreadable or foreign files are skipped; transcripts are best-effort data.
    let text = std::fs::read_to_string(p).ok()?;
    let mut st = if job.agent == "claude" {
        claude::parse(&text)
    } else {
        codex::parse(&text)
    };
    st.agent = job.agent;
    st.session = session_id(&job.root, p, job.agent);
    st.is_subagent = job.agent == "claude"
        && p.parent()
            .and_then(|d| d.file_name())
            .is_some_and(|n| n == "subagents");
    if st.is_subagent {
        let meta: serde_json::Value = std::fs::read_to_string(p.with_extension("meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        st.role = meta["agentType"].as_str().unwrap_or_default().to_owned();
        st.spawn_id = meta["toolUseId"].as_str().map(str::to_owned);
    } else {
        st.role = "main".into();
    }
    Some(st)
}
