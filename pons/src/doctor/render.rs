// SPDX-License-Identifier: MPL-2.0
//! Renders the doctor report as text or JSON.

use super::*;

pub(super) fn human_secs(s: u64) -> String {
    match s {
        0..120 => format!("{s}s"),
        120..7200 => format!("{}m", s / 60),
        7200..172_800 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

pub(super) fn human_bytes(n: u64) -> String {
    match n {
        0..10_240 => format!("{n} bytes"),
        10_240..10_485_760 => format!("{} KiB", n / 1024),
        _ => format!("{} MiB", n / 1_048_576),
    }
}

/// Renders the report as plain text. `colour` colours the adapter status (human
/// output on a terminal only).
pub fn render_text(r: &Report, colour: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "pons self doctor");
    let _ = writeln!(out, "  pons {} on {} {}", r.version, r.platform, r.arch);
    launcher_section(&mut out, r, colour);
    zed_section(&mut out, r);
    processes_section(&mut out, r);
    wire_log_section(&mut out, r);
    out
}

pub(super) fn launcher_section(out: &mut String, r: &Report, colour: bool) {
    let _ = writeln!(out, "\nLauncher");
    if !r.explicit_command {
        let _ = writeln!(out, "  no explicit command: pons would auto-detect");
    }
    match &r.would_run {
        Ok((how, cmd)) => {
            let _ = writeln!(out, "  would run: {how}");
            let _ = writeln!(out, "  command: {cmd}");
        }
        Err(e) => {
            let _ = writeln!(out, "  would run: nothing ({e})");
        }
    }
    let _ = writeln!(out, "  candidates, in order:");
    for (name, found) in &r.candidates {
        let _ = writeln!(
            out,
            "    {name}: {}",
            found.as_deref().unwrap_or("not found")
        );
    }
    let _ = writeln!(
        out,
        "  searched {} directories (PATH, then user bin directories)",
        r.searched_dirs
    );
    let status = match (r.adapter.status, colour) {
        ("ok", true) => "\x1b[32mok\x1b[0m",
        ("fail", true) => "\x1b[31mfail\x1b[0m",
        _ => r.adapter.status,
    };
    let _ = writeln!(
        out,
        "  claude-agent-acp adapter: {status}, {}",
        r.adapter.detail
    );
    if let Some(hint) = &r.adapter.hint {
        let _ = writeln!(out, "    hint: {hint}");
    }
}

/// Renders the report as one JSON object (ASCII: `serde_json` escapes the rest).
pub fn render_json(r: &Report) -> String {
    let settings = match &r.settings {
        None => Value::Null,
        Some(Settings::Servers(list)) => json!({
            "servers": list.iter().map(|s| json!({
                "name": s.name, "command": s.command, "args": s.args,
            })).collect::<Vec<_>>(),
        }),
        Some(Settings::Unparsed { mentions }) => json!({"unparsed": true, "mentions": mentions}),
    };
    let log = r.log.as_ref().map(|l| {
        let summary = l.summary.as_ref().map(|s| {
            json!({
                "lines": s.lines,
                "cancels_seen": s.cancels_seen,
                "cancels_dropped": s.cancels_dropped,
                "cancels_forwarded": s.cancels_forwarded(),
                "stop_escapes": s.stop_escapes,
                "subagents_tracked": s.subagents_tracked,
                "subagents_finished": s.subagents_finished,
                "subagents_provisional": s.subagents_provisional,
                "fail_safe_events": s.fail_safe_events,
                "notifications_dropped": s.notifications_dropped,
                "stepped_aside": s.stepped_aside,
                "last_exit": s.last_exit,
                "last_notes": s.last_notes,
            })
        });
        json!({
            "path": l.path,
            "size": l.size,
            "rotated_size": l.rotated_size,
            "summary": summary,
        })
    });
    let processes: Vec<Value> = r
        .processes
        .iter()
        .map(
            |p| json!({"pid": p.pid, "age_secs": p.age_secs, "stale": is_stale(p, r.exe_age_secs)}),
        )
        .collect();
    let (would_run, command, error) = match &r.would_run {
        Ok((how, cmd)) => (json!(how), json!(cmd), Value::Null),
        Err(e) => (Value::Null, Value::Null, json!(e)),
    };
    let v = json!({
        "pons": {"version": r.version, "platform": r.platform, "arch": r.arch},
        "launcher": {
            "explicit_command": r.explicit_command,
            "would_run": would_run,
            "command": command,
            "error": error,
            "candidates": r.candidates.iter().map(|(n, p)| json!({"name": n, "path": p})).collect::<Vec<_>>(),
            "searched_dirs": r.searched_dirs,
            "adapter": {"status": r.adapter.status, "detail": r.adapter.detail, "hint": r.adapter.hint},
            "configured_adapter": r.configured_adapter.as_ref().map(|(v, m)| json!({"version": v, "manifest": m})),
        },
        "zed": {
            "settings_path": r.settings_path,
            "settings": settings,
            "registry_adapter": r.registry_adapter.as_ref().map(|(v, d)| json!({"version": v, "dir": d})),
        },
        "processes": processes,
        "exe_age_secs": r.exe_age_secs,
        "log": log,
    });
    let mut text = serde_json::to_string_pretty(&v).unwrap_or_default();
    text.push('\n');
    text
}
