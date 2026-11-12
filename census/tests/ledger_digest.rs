// SPDX-License-Identifier: MPL-2.0
#![expect(
    clippy::unwrap_used,
    reason = "helpers outside test functions may panic"
)]
//! Checks that the digest folded from appended rows equals the digest built from the files, and
//! that every kind of mismatch makes the next read build it again.

use census::ledger::{Ledger, Opts, Priced};
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Barrier;

const BASE: u64 = 1_792_583_740;
const SENTINEL: f64 = 999.0;

struct Env {
    _tmp: tempfile::TempDir,
    projects: PathBuf,
    state: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    let state = tmp.path().join("state");
    Env {
        _tmp: tmp,
        projects,
        state,
    }
}

impl Env {
    fn update(&self, now: u64) -> Ledger {
        let opts = Opts::new(vec![self.projects.clone()], self.state.clone(), now);
        Ledger::open_and_update(&opts).unwrap()
    }

    fn digest_path(&self) -> PathBuf {
        self.state.join("digest.json")
    }

    fn digest(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.digest_path()).unwrap()).unwrap()
    }

    fn write_digest(&self, digest: &Value) {
        std::fs::write(self.digest_path(), serde_json::to_vec(digest).unwrap()).unwrap();
    }

    /// Marks the first raw row of the digest, so a later read shows whether it was built again.
    fn mark_digest(&self) {
        let mut digest = self.digest();
        digest["raw"][0][2] = json!(SENTINEL);
        self.write_digest(&digest);
    }

    /// True when the digest on disk was read as it is, with its mark.
    fn digest_is_trusted(&self) -> bool {
        Ledger::open(&self.state)
            .priced()
            .unwrap()
            .raw
            .iter()
            .any(|(_, usd)| (*usd - SENTINEL).abs() < f64::EPSILON)
    }

    fn rows_path(&self) -> PathBuf {
        self.state.join("rows.jsonl")
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

fn rec(id: &str, t: u64, model: &str, input: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{}","sessionId":"s1","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"cache_read_input_tokens":10,"output_tokens":3}}}}}}"#,
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

/// Asserts that the digest this ledger holds and the one on disk price what a build from the
/// ledger files prices, field by field.
fn assert_matches_rebuild(e: &Env, l: &Ledger) {
    let built = Priced::of(l.rows().unwrap(), l.hourly().unwrap());
    let held = l.priced().unwrap();
    assert_eq!(held.raw, built.raw);
    assert_eq!(held.hourly, built.hourly);
    let on_disk = Ledger::open(&e.state).priced().unwrap();
    assert_eq!(on_disk.raw, built.raw);
    assert_eq!(on_disk.hourly, built.hourly);
}

/// Deterministic generator, so a failing sequence repeats.
struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

#[test]
fn folding_appended_rows_equals_a_full_rebuild_over_generated_sequences() {
    const MODELS: [&str; 4] = [
        "claude-opus-5",
        "claude-haiku-5-5",
        "deepseek-flash",
        "unpriced-model",
    ];
    for seed in 1..=40_u64 {
        let mut rng = Rng(seed * 0x9e37_79b9 + 1);
        let e = env();
        let now = BASE + 20 * 86_400;
        let mut next_id = 0;
        let mut clock = now;
        for _batch in 0..=rng.next(6) {
            for _ in 0..rng.next(8) {
                let file = e.projects.join(format!("p/s{}.jsonl", rng.next(3)));
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                let age = if rng.next(4) == 0 {
                    rng.next(19 * 86_400)
                } else {
                    rng.next(6 * 86_400)
                };
                let model = MODELS[usize::try_from(rng.next(4)).unwrap()];
                let id = format!("m{}", if rng.next(9) == 0 { 0 } else { next_id });
                next_id += 1;
                append(&file, &rec(&id, now - age, model, 1 + rng.next(5000)));
            }
            clock += rng.next(7200);
            let l = e.update(clock);
            assert_matches_rebuild(&e, &l);
        }
    }
}

#[test]
fn a_current_digest_is_used_as_it_is() {
    let e = env();
    append(
        &e.projects.join("a.jsonl"),
        &rec("a", BASE, "claude-opus-5", 100),
    );
    e.update(BASE + 60);
    e.mark_digest();
    assert!(e.digest_is_trusted());
}

#[test]
fn rows_appended_to_a_current_digest_fold_in_without_a_rebuild() {
    let e = env();
    let file = e.projects.join("a.jsonl");
    append(&file, &rec("a", BASE, "claude-opus-5", 100));
    e.update(BASE + 60);
    e.mark_digest();
    append(&file, &rec("b", BASE + 30, "claude-opus-5", 200));
    let l = e.update(BASE + 90);
    assert_eq!(l.priced().unwrap().raw.len(), 2);
    assert!(e.digest_is_trusted());
}

#[test]
fn a_missing_digest_is_built_again() {
    let e = env();
    append(
        &e.projects.join("a.jsonl"),
        &rec("a", BASE, "claude-opus-5", 100),
    );
    let l = e.update(BASE + 60);
    std::fs::remove_file(e.digest_path()).unwrap();
    assert_matches_rebuild(&e, &l);
    assert!(e.digest_path().exists());
}

#[test]
fn a_corrupt_digest_is_built_again() {
    let e = env();
    append(
        &e.projects.join("a.jsonl"),
        &rec("a", BASE, "claude-opus-5", 100),
    );
    let l = e.update(BASE + 60);
    std::fs::write(e.digest_path(), b"{\"format\":3,").unwrap();
    assert_matches_rebuild(&e, &l);
}

#[test]
fn a_digest_of_another_format_or_version_is_built_again() {
    for (key, value) in [("format", json!(1)), ("census", json!("0.0.0"))] {
        let e = env();
        append(
            &e.projects.join("a.jsonl"),
            &rec("a", BASE, "claude-opus-5", 100),
        );
        e.update(BASE + 60);
        e.mark_digest();
        let mut digest = e.digest();
        digest[key] = value;
        e.write_digest(&digest);
        assert!(!e.digest_is_trusted(), "{key}");
    }
}

#[test]
fn a_digest_of_another_price_list_is_built_again() {
    let e = env();
    append(
        &e.projects.join("a.jsonl"),
        &rec("a", BASE, "claude-opus-5", 100),
    );
    e.update(BASE + 60);
    e.mark_digest();
    let mut digest = e.digest();
    digest["prices"] = json!(digest["prices"].as_u64().unwrap() + 1);
    e.write_digest(&digest);
    assert!(!e.digest_is_trusted());
}

#[test]
fn a_truncated_rows_file_is_built_again() {
    let e = env();
    let file = e.projects.join("a.jsonl");
    append(
        &file,
        &(rec("a", BASE, "claude-opus-5", 100) + &rec("b", BASE + 5, "claude-opus-5", 200)),
    );
    e.update(BASE + 60);
    e.mark_digest();
    let rows = std::fs::read(e.rows_path()).unwrap();
    let first_line = rows.iter().position(|b| *b == b'\n').unwrap() + 1;
    std::fs::write(e.rows_path(), &rows[..first_line]).unwrap();
    let l = Ledger::open(&e.state);
    assert_eq!(l.priced().unwrap().raw.len(), 1);
    assert!(!e.digest_is_trusted());
}

#[test]
fn a_rows_file_replaced_by_one_of_the_same_length_is_built_again() {
    let e = env();
    append(
        &e.projects.join("a.jsonl"),
        &rec("a", BASE, "claude-opus-5", 100),
    );
    e.update(BASE + 60);
    e.mark_digest();
    let mut rows = std::fs::read(e.rows_path()).unwrap();
    let at = rows.len() - 3;
    rows[at] = if rows[at] == b'7' { b'8' } else { b'7' };
    std::fs::write(e.rows_path(), &rows).unwrap();
    assert!(!e.digest_is_trusted());
}

#[test]
fn a_changed_hourly_file_is_built_again() {
    let e = env();
    append(
        &e.projects.join("a.jsonl"),
        &rec("a", BASE, "claude-opus-5", 100),
    );
    e.update(BASE + 60);
    e.mark_digest();
    append(&e.state.join("hourly.jsonl"), "\n");
    assert!(!e.digest_is_trusted());
}

#[test]
fn concurrent_updates_count_each_row_once() {
    let e = env();
    let file = e.projects.join("a.jsonl");
    let mut text = String::new();
    for i in 0..200 {
        text += &rec(&format!("m{i}"), BASE + i, "claude-opus-5", 10 + i);
    }
    append(&file, &text);
    e.update(BASE + 300);
    for round in 0..10 {
        let mut more = String::new();
        for i in 0..20 {
            more += &rec(
                &format!("r{round}-{i}"),
                BASE + 300 + round * 20 + i,
                "claude-opus-5",
                50,
            );
        }
        append(&file, &more);
        let gate = Barrier::new(4);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    gate.wait();
                    let opts = Opts::new(vec![e.projects.clone()], e.state.clone(), BASE + 1000);
                    let _ = Ledger::open_and_update(&opts);
                });
            }
        });
        let l = e.update(BASE + 1000);
        assert_eq!(l.rows().unwrap().len(), 200 + 20 * (round as usize + 1));
        assert_matches_rebuild(&e, &l);
    }
}
