// SPDX-License-Identifier: MPL-2.0
//! The doctor result in the shared doctor model: four groups of checks, and the pons facts
//! that are not a check as detail lines.

use capsa_core::exit::Outcome;
use capsa_core::render::{Check, Doc, Doctor, Group, Render, Status};
use serde::{Serialize, Serializer};

use super::*;

/// What `pons self doctor` shows: the checks, and the facts behind them.
pub struct View {
    report: Report,
    doctor: Doctor,
}

impl View {
    /// Builds the view of `report`.
    pub fn new(report: Report) -> Self {
        let doctor = Doctor::new("pons", report.version, groups(&report));
        Self { report, doctor }
    }
}

impl Render for View {
    fn doc(&self, doc: &mut Doc) {
        self.doctor.doc(doc);
        details(&self.report, doc);
    }

    fn outcome(&self) -> Outcome {
        self.doctor.outcome()
    }
}

/// Serializes as the shared doctor model: `tool`, `version`, `ok` and `groups`.
impl Serialize for View {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.doctor.serialize(serializer)
    }
}

fn check(
    name: &'static str,
    status: Status,
    detail: impl Into<String>,
    hint: Option<String>,
) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
        hint,
    }
}

/// The four groups of checks: launcher, zed, processes and wire log.
fn groups(r: &Report) -> Vec<Group> {
    vec![
        Group {
            name: "launcher",
            checks: launcher_checks(r),
        },
        Group {
            name: "zed",
            checks: zed_checks(r),
        },
        Group {
            name: "processes",
            checks: process_checks(r),
        },
        Group {
            name: "wire log",
            checks: log_checks(r),
        },
    ]
}

fn launcher_checks(r: &Report) -> Vec<Check> {
    let (status, detail) = match &r.would_run {
        Ok((how, cmd)) => (Status::Ok, format!("{how}: {cmd}")),
        Err(e) => (Status::Fail, format!("nothing would run: {e}")),
    };
    let adapter = &r.adapter;
    let adapter_status = if adapter.status == "ok" {
        Status::Ok
    } else {
        Status::Fail
    };
    vec![
        check("would run", status, detail, None),
        check(
            "claude-agent-acp adapter",
            adapter_status,
            adapter.detail.clone(),
            adapter.hint.clone(),
        ),
    ]
}

fn zed_checks(r: &Report) -> Vec<Check> {
    let (status, detail) = match (&r.settings_path, &r.settings) {
        (None, _) => (
            Status::Warn,
            "settings.json location unknown (no home directory)".to_owned(),
        ),
        (Some(path), None) => (Status::Warn, format!("{path}: not found or unreadable")),
        (Some(path), Some(Settings::Unparsed { mentions })) => (
            Status::Warn,
            format!("{path}: could not parse; {mentions} line(s) mention pons"),
        ),
        (Some(path), Some(Settings::Servers(list))) if list.is_empty() => (
            Status::Warn,
            format!("{path}: no agent_servers entry runs pons"),
        ),
        (Some(path), Some(Settings::Servers(list))) => {
            let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
            (
                Status::Ok,
                format!("{path}: runs pons as {}", names.join(", ")),
            )
        }
    };
    vec![check("agent_servers entry", status, detail, None)]
}

fn process_checks(r: &Report) -> Vec<Check> {
    let running = r.processes.len();
    let stale = r
        .processes
        .iter()
        .filter(|p| is_stale(p, r.exe_age_secs))
        .count();
    let row = if running == 0 {
        check(
            "running proxies",
            Status::Ok,
            "no running pons proxy found",
            None,
        )
    } else if stale == 0 {
        check(
            "running proxies",
            Status::Ok,
            format!("{running} running, none started before the installed binary changed"),
            None,
        )
    } else {
        check(
            "running proxies",
            Status::Warn,
            format!(
                "{running} running, {stale} stale: started before the installed binary changed"
            ),
            Some("restart the agent in Zed".to_owned()),
        )
    };
    vec![row]
}

fn log_checks(r: &Report) -> Vec<Check> {
    let row = match &r.log {
        None => check(
            "file",
            Status::Warn,
            "none: no pons-*.jsonl in the log directory",
            Some("pass --log FILE, or set PONS_LOG".to_owned()),
        ),
        Some(log) => match &log.summary {
            None => check(
                "file",
                Status::Warn,
                format!("{}: not found or unreadable", log.path),
                None,
            ),
            Some(s) if s.legacy_text => check(
                "file",
                Status::Ok,
                format!("{}: old text log, not summarized", log.path),
                None,
            ),
            Some(s) => {
                let status = if s.fail_safe_events > 0 {
                    Status::Warn
                } else {
                    Status::Ok
                };
                check(
                    "file",
                    status,
                    format!(
                        "{} ({} lines, {} fail-safe events)",
                        log.path, s.lines, s.fail_safe_events
                    ),
                    None,
                )
            }
        },
    };
    vec![row]
}

/// The facts the checks summarize: the launcher's candidates, the Zed entries, the running
/// proxies and the wire log.
fn details(r: &Report, doc: &mut Doc) {
    doc.section("details");
    doc.line(format!("pons {} on {} {}", r.version, r.platform, r.arch));
    if !r.explicit_command {
        doc.line("no explicit command: pons would auto-detect");
    }
    doc.line("candidates, in order:");
    for (name, found) in &r.candidates {
        doc.line(format!(
            "{name}: {}",
            found.as_deref().unwrap_or("not found")
        ));
    }
    doc.line(format!(
        "searched {} directories (PATH, then user bin directories)",
        r.searched_dirs
    ));
    if let Some(Settings::Servers(list)) = &r.settings {
        for s in list {
            doc.line(format!(
                "agent_servers {}: command {:?}, args {:?}",
                s.name, s.command, s.args
            ));
        }
    }
    if let Some((version, dir)) = &r.registry_adapter {
        let version = version.as_deref().unwrap_or("not found");
        doc.line(format!(
            "Zed registry adapter (a separate install, not what pons runs): {version} ({dir})"
        ));
    }
    for p in &r.processes {
        let stale = if is_stale(p, r.exe_age_secs) {
            " (stale)"
        } else {
            ""
        };
        doc.line(format!(
            "pid {}: running for {}{stale}",
            p.pid,
            human_secs(p.age_secs)
        ));
    }
    if let Some(age) = r.exe_age_secs {
        doc.line(format!("installed binary changed {} ago", human_secs(age)));
    }
    if let Some(log) = &r.log {
        let size = log.size.map_or_else(|| "unknown".to_owned(), human_bytes);
        let rotated = log.rotated_size.map_or_else(
            || "not rotated yet".to_owned(),
            |old| format!("{}.1 is {}", log.path, human_bytes(old)),
        );
        doc.line(format!("log size: {size}; {rotated}"));
        if let Some(s) = &log.summary {
            doc.line(format!(
                "cancels: {} seen, {} dropped, {} forwarded, {} stop escapes",
                s.cancels_seen,
                s.cancels_dropped,
                s.cancels_forwarded(),
                s.stop_escapes
            ));
            doc.line(format!(
                "subagents: {} tracked, {} finished ({} of them provisional, by silence)",
                s.subagents_tracked, s.subagents_finished, s.subagents_provisional
            ));
            doc.line(format!(
                "task notifications dropped: {}",
                s.notifications_dropped
            ));
            doc.line(format!(
                "client renders subagents natively (pons stepped aside): {}",
                if s.stepped_aside { "yes" } else { "no" }
            ));
            doc.line(format!(
                "last exit: {}",
                s.last_exit.as_deref().unwrap_or("none logged")
            ));
            if s.last_notes.is_empty() {
                doc.line("last notes: none");
            } else {
                doc.line(format!("last {} note(s):", s.last_notes.len()));
                for note in &s.last_notes {
                    doc.line(note.as_str());
                }
            }
        }
    }
}
