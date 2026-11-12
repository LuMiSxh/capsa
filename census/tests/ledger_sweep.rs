// SPDX-License-Identifier: MPL-2.0
#![expect(
    clippy::unwrap_used,
    reason = "helpers outside test functions may panic"
)]

use census::ledger::{Filter, Ledger, Opts};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const BASE: u64 = 1_792_583_740;

struct Env {
    _tmp: tempfile::TempDir,
    projects: PathBuf,
    state: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects/p");
    std::fs::create_dir_all(&projects).unwrap();
    let state = tmp.path().join("state");
    Env {
        _tmp: tmp,
        projects,
        state,
    }
}

impl Env {
    fn opts(&self, now: u64, current: Option<&Path>) -> Opts {
        Opts {
            current: current.map(Path::to_owned),
            ..Opts::new(
                vec![self.projects.parent().unwrap().to_owned()],
                self.state.clone(),
                now,
            )
        }
    }

    fn update(&self, now: u64, current: Option<&Path>) -> Ledger {
        Ledger::open_and_update(&self.opts(now, current)).unwrap()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.projects.join(name)
    }
}

fn iso(t: u64) -> String {
    let (days, secs) = (t / 86_400, t % 86_400);
    let z = i64::try_from(days).unwrap() + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn rec(id: &str, t: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{}","sessionId":"s1","message":{{"id":"{id}","model":"claude-opus-5","usage":{{"input_tokens":1,"output_tokens":3}}}}}}"#,
        iso(t)
    ) + "\n"
}

fn append(path: &Path, text: &str) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn ids(l: &Ledger) -> Vec<String> {
    let mut ids: Vec<String> = l.rows().unwrap().iter().map(|r| r.id.clone()).collect();
    ids.sort();
    ids
}

#[test]
fn inside_the_bound_only_the_current_transcript_is_read() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&cur, &rec("c1", BASE));
    append(&other, &rec("o1", BASE));
    e.update(BASE, None);

    append(&other, &rec("o2", BASE + 5));
    append(&cur, &rec("c2", BASE + 5));
    let within = e.update(BASE + 59, Some(&cur));
    assert_eq!(within.report().files_scanned, 1);
    assert_eq!(ids(&within), ["c1", "c2", "o1"]);

    let at_bound = e.update(BASE + 60, Some(&cur));
    assert_eq!(ids(&at_bound), ["c1", "c2", "o1", "o2"]);
}

#[test]
fn a_row_in_two_transcripts_counts_once_across_fast_and_full_updates() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&other, &rec("shared", BASE));
    e.update(BASE, None);

    append(&cur, &(rec("shared", BASE + 1) + &rec("c1", BASE + 1)));
    let fast = e.update(BASE + 2, Some(&cur));
    assert_eq!(ids(&fast), ["c1", "shared"]);
    assert_eq!(fast.report().rows_added, 1);

    append(&other, &rec("o2", BASE + 3));
    let full = e.update(BASE + 3, None);
    assert_eq!(ids(&full), ["c1", "o2", "shared"]);
    assert_eq!(full.report().rows_added, 1);
    assert_eq!(full.tokens(&Filter::default()).unwrap().requests, 3);
}

#[test]
fn a_missing_sweep_time_means_a_full_sweep() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&cur, &rec("c1", BASE));
    append(&other, &rec("o1", BASE));
    e.update(BASE, None);
    let index = e.state.join("index.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("swept_at").unwrap();
    std::fs::write(&index, value.to_string()).unwrap();

    append(&other, &rec("o2", BASE + 5));
    let l = e.update(BASE + 5, Some(&cur));
    assert_eq!(ids(&l), ["c1", "o1", "o2"]);
}

#[test]
fn a_torn_index_means_a_full_sweep_even_with_a_recent_sweep_time() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&cur, &rec("c1", BASE));
    append(&other, &rec("o1", BASE));
    e.update(BASE, None);
    std::fs::write(
        e.state.join("index.json"),
        b"{\"version\":1,\"oldest_t\":null,\"swept_at\":1792583740,\"files\":{",
    )
    .unwrap();

    append(&other, &rec("o2", BASE + 5));
    let l = e.update(BASE + 5, Some(&cur));
    assert_eq!(ids(&l), ["c1", "o1", "o2"]);
}

#[test]
fn a_partial_sweep_does_not_open_the_fresh_window() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&cur, &rec("c1", BASE));
    append(&other, &rec("o1", BASE));
    let partial = Ledger::open_and_update(&Opts {
        deadline: Some(Instant::now()),
        ..e.opts(BASE, None)
    })
    .unwrap();
    assert!(partial.report().partial);

    let l = e.update(BASE + 5, Some(&cur));
    assert_eq!(ids(&l), ["c1", "o1"]);
}

#[test]
fn a_sweep_time_after_now_means_a_full_sweep() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&cur, &rec("c1", BASE));
    append(&other, &rec("o1", BASE));
    e.update(BASE + 100, None);

    append(&other, &rec("o2", BASE + 5));
    let l = e.update(BASE + 50, Some(&cur));
    assert_eq!(ids(&l), ["c1", "o1", "o2"]);
}

#[test]
fn a_current_transcript_outside_the_roots_means_a_full_sweep() {
    let e = env();
    let (cur, other) = (e.file("cur.jsonl"), e.file("other.jsonl"));
    append(&cur, &rec("c1", BASE));
    append(&other, &rec("o1", BASE));
    e.update(BASE, None);

    let outside = e._tmp.path().join("outside.jsonl");
    append(&outside, &rec("x1", BASE));
    append(&other, &rec("o2", BASE + 5));
    let l = e.update(BASE + 5, Some(&outside));
    assert_eq!(ids(&l), ["c1", "o1", "o2"]);
}
