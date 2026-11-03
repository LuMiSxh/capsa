// SPDX-License-Identifier: MPL-2.0
#![expect(
    clippy::unwrap_used,
    reason = "helpers outside test functions may panic"
)]

use capsa_core::census_snapshot::{self, Snapshot, Source, Window};
use census::ledger::{Filter, Ledger, Opts, Priced};
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
    fn opts(&self, now: u64) -> Opts {
        Opts::new(
            vec![self.projects.parent().unwrap().to_owned()],
            self.state.clone(),
            now,
        )
    }

    fn update(&self, now: u64) -> Ledger {
        Ledger::open_and_update(&self.opts(now)).unwrap()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.projects.join(name)
    }
}

fn iso(t: u64) -> String {
    let (days, rem) = (t / 86_400, t % 86_400);
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
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A request of `model` instead of the Claude model `rec` writes.
fn api_rec(id: &str, t: u64, model: &str) -> String {
    rec(id, t, None).replace("claude-opus-5", model)
}

fn rec(id: &str, t: u64, agent: Option<(&str, &str)>) -> String {
    let who = agent.map_or(String::new(), |(a, r)| {
        format!(r#""isSidechain":true,"agentId":"{a}","attributionAgent":"{r}","#)
    });
    format!(
        r#"{{{who}"type":"assistant","timestamp":"{}","sessionId":"s1","message":{{"id":"{id}","model":"claude-opus-5","usage":{{"input_tokens":1,"cache_read_input_tokens":10,"cache_creation_input_tokens":6,"cache_creation":{{"ephemeral_5m_input_tokens":4,"ephemeral_1h_input_tokens":2}},"output_tokens":3}}}}}}"#,
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

fn total(l: &Ledger) -> census::ledger::Tokens {
    l.tokens(&Filter::default()).unwrap()
}

/// Asserts that the digest prices the same rows and aggregates as the ledger files.
fn assert_digest_matches(l: &Ledger) {
    assert_eq!(
        l.priced().unwrap(),
        Priced::of(l.rows().unwrap(), l.hourly().unwrap())
    );
}

#[test]
fn digest_matches_the_files_through_appends_folds_and_loss() {
    let e = env();
    let recent = BASE + 19 * 86_400;
    let now = BASE + 20 * 86_400;
    append(
        &e.file("a.jsonl"),
        &(rec("old", BASE, None) + &rec("new", recent, None)),
    );
    let l = e.update(now);
    assert_digest_matches(&l);
    assert!(e.state.join("digest.json").exists());

    append(&e.file("a.jsonl"), &rec("later", now, None));
    let again = e.update(now + 60);
    assert_digest_matches(&again);
    assert_eq!(again.priced().unwrap().raw.len(), 2);

    std::fs::remove_file(e.state.join("digest.json")).unwrap();
    assert_digest_matches(&Ledger::open(&e.state));
    assert!(e.state.join("digest.json").exists());

    std::fs::write(e.state.join("digest.json"), b"{not json").unwrap();
    assert_digest_matches(&Ledger::open(&e.state));
}

#[test]
fn requests_of_other_providers_stay_out_of_the_limits_and_the_priced_rows() {
    let now = BASE + 3600;
    let mixed = env();
    append(
        &mixed.file("a.jsonl"),
        &(rec("claude", BASE, None)
            + &api_rec("ds", BASE + 60, "deepseek-flash")
            + &api_rec("gpt", BASE + 120, "gpt-5")),
    );
    let l = mixed.update(now);
    assert_digest_matches(&l);
    let priced = l.priced().unwrap();
    assert_eq!(priced.raw.iter().map(|x| x.0).collect::<Vec<_>>(), [BASE]);

    let only = env();
    append(&only.file("a.jsonl"), &rec("claude", BASE, None));
    let claude = only.update(now).priced().unwrap();
    let burn = |p: &Priced| {
        census::limits::compute(
            &census::limits::History {
                priced: p,
                snapshots: &[],
            },
            now,
        )
        .burn
    };
    assert!(burn(&claude).usd_per_hour_60 > 0.0);
    assert_eq!(burn(&priced), burn(&claude));
}

#[test]
fn a_digest_without_the_provider_split_is_rebuilt() {
    let e = env();
    append(
        &e.file("a.jsonl"),
        &(rec("claude", BASE, None) + &api_rec("ds", BASE + 60, "deepseek-flash")),
    );
    let l = e.update(BASE + 3600);
    let path = e.state.join("digest.json");
    let mut digest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    digest["raw"][1][2] = serde_json::json!(1.0);
    digest.as_object_mut().unwrap().remove("format");
    std::fs::write(&path, serde_json::to_vec(&digest).unwrap()).unwrap();

    let rebuilt = Ledger::open(&e.state).priced().unwrap();
    assert_eq!(rebuilt.raw.len(), 1);
    assert_eq!(rebuilt, Priced::of(l.rows().unwrap(), l.hourly().unwrap()));
}

#[test]
fn update_reads_only_appended_bytes() {
    let e = env();
    let f = e.file("a.jsonl");
    append(&f, &(rec("m1", BASE, None) + &rec("m2", BASE + 5, None)));
    let first = e.update(BASE + 10);
    assert_eq!(first.rows().unwrap().len(), 2);
    let size = std::fs::metadata(&f).unwrap().len();
    assert_eq!(first.report().bytes_read, size);

    let warm = e.update(BASE + 10);
    assert_eq!(
        (warm.report().bytes_read, warm.report().files_scanned),
        (0, 0)
    );

    let extra = rec("m3", BASE + 20, None);
    append(&f, &extra);
    let next = e.update(BASE + 30);
    assert_eq!(next.report().bytes_read, extra.len() as u64);
    assert_eq!(next.rows().unwrap().len(), 3);
    assert_eq!(total(&next).requests, 3);
}

#[test]
fn rows_carry_token_classes_and_gap() {
    let e = env();
    append(
        &e.file("a.jsonl"),
        &(rec("m1", BASE, None) + &rec("m2", BASE + 7, None)),
    );
    let l = e.update(BASE + 10);
    let rows = l.rows().unwrap();
    let second = rows.iter().find(|r| r.id == "m2").unwrap();
    assert_eq!(second.gap_s, Some(7));
    assert_eq!(rows.iter().find(|r| r.id == "m1").unwrap().gap_s, None);
    assert_eq!(
        (
            second.input,
            second.cache_read,
            second.cache_write_5m,
            second.cache_write_1h,
            second.output
        ),
        (1, 10, 4, 2, 3)
    );
}

#[test]
fn partial_last_line_is_consumed_only_when_complete() {
    let e = env();
    let f = e.file("a.jsonl");
    let second = rec("m2", BASE + 5, None);
    let (head, tail) = second.trim_end().split_at(40);
    append(&f, &(rec("m1", BASE, None) + head));
    assert_eq!(e.update(BASE + 10).rows().unwrap().len(), 1);

    append(&f, &format!("{tail}\n"));
    let l = e.update(BASE + 10);
    assert_eq!(l.rows().unwrap().len(), 2);
    assert_eq!(l.report().bytes_read, second.len() as u64);
}

#[test]
fn duplicate_message_ids_count_once_across_records_and_files() {
    let e = env();
    append(
        &e.file("a.jsonl"),
        &(rec("m1", BASE, None) + &rec("m1", BASE + 1, None)),
    );
    append(&e.file("b.jsonl"), &rec("m1", BASE + 2, None));
    let l = e.update(BASE + 10);
    assert_eq!(total(&l).requests, 1);
    assert_eq!(total(&e.update(BASE + 20)).requests, 1);
}

#[test]
fn truncated_or_replaced_file_is_rescanned_without_double_counting() {
    let e = env();
    let f = e.file("a.jsonl");
    append(
        &f,
        &(rec("m1", BASE, None) + &rec("m2", BASE + 5, None) + &rec("m3", BASE + 6, None)),
    );
    assert_eq!(total(&e.update(BASE + 10)).requests, 3);

    std::fs::write(&f, rec("m1", BASE, None) + &rec("m4", BASE + 8, None)).unwrap();
    let l = e.update(BASE + 20);
    assert!(l.report().bytes_read > 0);
    let mut ids: Vec<_> = l.rows().unwrap().iter().map(|r| r.id.clone()).collect();
    ids.sort();
    assert_eq!(ids, ["m1", "m2", "m3", "m4"]);

    std::fs::remove_file(&f).unwrap();
    append(&f, &rec("m1", BASE, None));
    assert_eq!(total(&e.update(BASE + 30)).requests, 4);
}

#[test]
fn subagent_identity_and_filters() {
    let e = env();
    let sub = e.file("s1/subagents/agent-a9.jsonl");
    std::fs::create_dir_all(sub.parent().unwrap()).unwrap();
    append(&e.file("a.jsonl"), &rec("m1", BASE, None));
    append(&sub, &rec("m2", BASE + 1, Some(("a9", "Explore"))));
    let l = e.update(BASE + 10);
    let row = l.rows().unwrap().iter().find(|r| r.id == "m2").unwrap();
    assert_eq!(
        (row.agent.as_deref(), row.role.as_deref()),
        (Some("a9"), Some("Explore"))
    );
    let only_sub = Filter {
        sub: Some(true),
        ..Filter::default()
    };
    assert_eq!(l.tokens(&only_sub).unwrap().requests, 1);
    let main = Filter {
        sub: Some(false),
        family: Some("opus".into()),
        ..Filter::default()
    };
    assert_eq!(l.tokens(&main).unwrap().requests, 1);
    let sonnet = Filter {
        family: Some("sonnet".into()),
        ..Filter::default()
    };
    assert_eq!(l.tokens(&sonnet).unwrap().requests, 0);
}

#[test]
fn rows_past_retention_fold_into_hourly_once() {
    let e = env();
    let old = BASE;
    let recent = BASE + 19 * 86_400;
    let now = BASE + 20 * 86_400;
    append(
        &e.file("a.jsonl"),
        &(rec("old", old, None) + &rec("new", recent, None)),
    );
    let l = e.update(now);
    assert_eq!(l.report().folded, 1);
    assert_eq!(l.rows().unwrap().len(), 1);
    assert_eq!(l.hourly().unwrap().len(), 1);
    assert_eq!(l.hourly().unwrap()[0].tokens.requests, 1);
    assert_eq!(total(&l).requests, 2);

    append(&e.file("a.jsonl"), &rec("later", now, None));
    let again = e.update(now + 60);
    assert_eq!(total(&again).requests, 3);
    assert_eq!(again.hourly().unwrap().len(), 1);

    std::fs::remove_file(e.state.join("index.json")).unwrap();
    assert_eq!(total(&e.update(now + 120)).requests, 3);
}

#[test]
fn rows_age_out_of_the_raw_file_as_time_passes() {
    let e = env();
    append(&e.file("a.jsonl"), &rec("m1", BASE, None));
    assert_eq!(e.update(BASE + 60).hourly().unwrap().len(), 0);
    let l = e.update(BASE + 9 * 86_400);
    assert_eq!((l.rows().unwrap().len(), l.hourly().unwrap().len()), (0, 1));
    assert_eq!(total(&l).requests, 1);
}

#[test]
fn prune_dry_run_reports_and_real_run_removes() {
    let e = env();
    append(
        &e.file("a.jsonl"),
        &(rec("old", BASE, None) + &rec("new", BASE + 19 * 86_400, None)),
    );
    let now = BASE + 20 * 86_400;
    let mut l = e.update(now);
    let snap = |t| Snapshot {
        t,
        source: Source::Statusline,
        window: Window::SevenDay,
        used_percentage: 40.0,
        resets_at: t + 1000,
    };
    census_snapshot::append_to(&e.state.join("snapshots"), &snap(BASE)).unwrap();
    census_snapshot::append_to(&e.state.join("snapshots"), &snap(now)).unwrap();

    let before = now - 8 * 86_400;
    let dry = l.prune(before, true).unwrap();
    assert_eq!(
        (dry.hourly_removed, dry.snapshots_removed, dry.dry_run),
        (1, 1, true)
    );
    assert_eq!(l.hourly().unwrap().len(), 1);

    let real = l.prune(before, false).unwrap();
    assert_eq!((real.hourly_removed, real.snapshots_removed), (1, 1));
    assert!(l.hourly().unwrap().is_empty());
    assert_eq!(
        census_snapshot::read_since_from(&e.state.join("snapshots"), 0)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(l.rows().unwrap().len(), 1);
}

#[test]
fn held_lock_returns_current_state_without_writing() {
    let e = env();
    let f = e.file("a.jsonl");
    append(&f, &rec("m1", BASE, None));
    assert_eq!(e.update(BASE + 10).rows().unwrap().len(), 1);

    append(&f, &rec("m2", BASE + 5, None));
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(e.state.join("lock"))
        .unwrap();
    lock.lock().unwrap();
    let l = e.update(BASE + 20);
    assert!(!l.report().updated);
    assert_eq!(l.rows().unwrap().len(), 1);
    drop(lock);

    let l = e.update(BASE + 30);
    assert!(l.report().updated);
    assert_eq!(l.rows().unwrap().len(), 2);
}

#[test]
fn rows_appended_before_a_lost_index_are_not_counted_twice() {
    let e = env();
    append(
        &e.file("a.jsonl"),
        &(rec("m1", BASE, None) + &rec("m2", BASE + 1, None)),
    );
    e.update(BASE + 10);
    std::fs::remove_file(e.state.join("index.json")).unwrap();
    let l = e.update(BASE + 20);
    assert_eq!(l.rows().unwrap().len(), 2);
    assert_eq!(l.report().rows_added, 0);
}

#[test]
fn torn_last_line_in_rows_is_cut_before_the_next_append() {
    let e = env();
    let f = e.file("a.jsonl");
    append(&f, &rec("m1", BASE, None));
    e.update(BASE + 10);
    append(&e.state.join("rows.jsonl"), r#"{"id":"torn","t":17"#);
    append(&f, &rec("m2", BASE + 5, None));
    let l = e.update(BASE + 20);
    assert_eq!(l.rows().unwrap().len(), 2);
    let text = std::fs::read_to_string(e.state.join("rows.jsonl")).unwrap();
    assert!(text.ends_with('\n'));
    assert_eq!(text.lines().count(), 2);
    assert!(text.lines().all(|l| l.starts_with('{') && l.ends_with('}')));
}

/// Updates with a deadline that has already passed until nothing is left; returns the update count.
fn drain(e: &Env, now: u64) -> usize {
    let mut runs = 0;
    loop {
        let opts = Opts {
            deadline: Some(Instant::now()),
            ..e.opts(now)
        };
        let l = Ledger::open_and_update(&opts).unwrap();
        runs += 1;
        assert!(l.report().bytes_read > 0 || !l.report().partial);
        if !l.report().partial {
            return runs;
        }
        assert!(runs < 100, "partial updates do not converge");
    }
}

#[test]
fn expired_deadline_keeps_progress_file_by_file_and_converges() {
    let e = env();
    for (i, name) in ["a", "b", "c"].iter().enumerate() {
        let t = BASE + i as u64 * 10;
        append(
            &e.file(&format!("{name}.jsonl")),
            &(rec(&format!("{name}1"), t, None) + &rec(&format!("{name}2"), t + 1, None)),
        );
    }
    let first = Ledger::open_and_update(&Opts {
        deadline: Some(Instant::now()),
        ..e.opts(BASE + 100)
    })
    .unwrap();
    assert!(first.report().partial);
    assert_eq!(first.report().files_scanned, 1);
    assert_eq!(first.rows().unwrap().len(), 2);

    assert!(drain(&e, BASE + 100) >= 2);
    let full = env();
    for name in ["a", "b", "c"] {
        std::fs::copy(
            e.file(&format!("{name}.jsonl")),
            full.file(&format!("{name}.jsonl")),
        )
        .unwrap();
    }
    let all = full.update(BASE + 100);
    assert!(!all.report().partial);
    let drained = e.update(BASE + 100);
    assert_eq!(total(&drained), total(&all));
    assert_eq!(total(&drained).requests, 6);
}

#[test]
fn expired_deadline_stops_inside_a_large_file_without_double_counting() {
    let e = env();
    let n = 20_000_u64;
    let body: String = (0..n)
        .map(|i| rec(&format!("m{i}"), BASE + i, None))
        .collect();
    append(&e.file("big.jsonl"), &body);
    let first = Ledger::open_and_update(&Opts {
        deadline: Some(Instant::now()),
        ..e.opts(BASE + n + 10)
    })
    .unwrap();
    assert!(first.report().partial);
    let got = first.rows().unwrap().len();
    assert!(got > 0 && (got as u64) < n);

    assert!(drain(&e, BASE + n + 10) >= 1);
    let done = e.update(BASE + n + 10);
    assert_eq!(total(&done).requests, n);
    assert_eq!(done.rows().unwrap().len() as u64, n);
}

#[test]
fn snapshot_file_skips_a_malformed_line() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("snapshots");
    let s = Snapshot {
        t: 5,
        source: Source::Pons,
        window: Window::FiveHour,
        used_percentage: 12.0,
        resets_at: 99,
    };
    census_snapshot::append_to(&file, &s).unwrap();
    append(&file, "not json\n");
    census_snapshot::append_to(&file, &Snapshot { t: 9, ..s }).unwrap();
    let got = census_snapshot::read_since_from(&file, 6).unwrap();
    assert_eq!(got, vec![Snapshot { t: 9, ..s }]);
}
