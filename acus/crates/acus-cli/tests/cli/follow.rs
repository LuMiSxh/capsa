// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

const LIB: &str = "pub fn parse(src: &str) -> usize {\n    src.len()\n}\n\npub fn run(src: &str) -> usize {\n    let n = parse(src);\n    n + check()\n}\n\nfn check() -> usize {\n    1\n}\n";
const MAIN: &str = "fn main() {\n    let n = run(\"x\");\n    println!(\"{n}\");\n}\n";

/// A fresh directory holding `files`.
fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(d.path().join(name), text).unwrap();
    }
    d
}

/// Addresses at the start of each printed row, skipping the `…` and `?` notes.
fn printed_addrs(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| !l.trim_start().starts_with(['…', '?']))
        .filter_map(|l| l.trim_start().split("  ").next())
        .filter(|a| a.contains('#') || a.contains(':'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn callers_follow_sure_edges_and_indent_by_level() {
    let d = fixture(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    let (code, out, err) = acus_in(d.path(), &["follow", "lib.rs#parse"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "lib.rs#parse  fn 1-3  callers depth 2\n  lib.rs#run  6  let n = parse(src);\n    main.rs#main  2  let n = run(\"x\");\n"
    );
}

#[test]
fn json_lists_edges_with_from_to_and_depth() {
    let d = fixture(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "lib.rs#parse", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["target"]["addr"].as_str(), Some("lib.rs#parse"));
    assert_eq!(v["target"]["kind"].as_str(), Some("fn"));
    assert_eq!(v["direction"].as_str(), Some("callers"));
    let edges = v["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2, "{out}");
    assert_eq!(edges[0]["from"].as_str(), Some("lib.rs#run"));
    assert_eq!(edges[0]["to"].as_str(), Some("lib.rs#parse"));
    assert_eq!(edges[0]["lines"], serde_json::json!([6]));
    assert_eq!(edges[0]["sure"], true);
    assert_eq!(edges[1]["from"].as_str(), Some("main.rs#main"));
    assert_eq!(edges[1]["to"].as_str(), Some("lib.rs#run"));
    assert_eq!(edges[1]["depth"], 2);
    assert_eq!(v["truncated"], serde_json::json!([]));
}

#[test]
fn method_calls_with_unknown_receiver_are_unsure_and_not_followed() {
    let method = "pub struct Store;\nimpl Store {\n    pub fn save(&self) -> u8 { 1 }\n}\npub fn flush(s: &Store) -> u8 {\n    s.save()\n}\npub fn top(s: &Store) -> u8 {\n    flush(s)\n}\n";
    let d = fixture(&[("method.rs", method)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "method.rs#Store::save"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "method.rs#Store::save  fn 3-3  callers depth 2\n  method.rs#flush  6  ?  s.save()\n"
    );
}

#[test]
fn macro_argument_calls_are_unsure() {
    let macros =
        "pub fn parse(x: u8) -> u8 { x }\n#[test]\nfn check() {\n    assert_eq!(parse(1), 1);\n}\n";
    let d = fixture(&[("macros.rs", macros)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "macros.rs#parse"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "macros.rs#parse  fn 1-1  callers depth 2\n  macros.rs#check  4  ?  assert_eq!(parse(1), 1);\n"
    );
}

#[test]
fn a_macro_call_is_not_a_caller_of_a_fn_of_the_same_name() {
    let lib = "pub fn format(x: u8) -> u8 {\n    x\n}\n";
    let main = "fn main() {\n    let s = format!(\"{}\", 1);\n    println!(\"{s}\");\n}\n";
    let d = fixture(&[("lib.rs", lib), ("main.rs", main)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "lib.rs#format"], "");
    assert_eq!(code, 1);
    assert_eq!(out, "lib.rs#format  fn 1-3  callers depth 2\n");
}

#[test]
fn down_macro_call_does_not_reach_a_fn_of_the_same_name() {
    let lib = "pub fn format(x: u8) -> u8 {\n    x\n}\n";
    let main = "fn main() {\n    let s = format!(\"{}\", 1);\n    println!(\"{s}\");\n}\n";
    let d = fixture(&[("lib.rs", lib), ("main.rs", main)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "main.rs#main", "--down"], "");
    assert_eq!(code, 1);
    assert_eq!(out, "main.rs#main  fn 1-4  callees depth 2\n");
}

#[test]
fn down_counts_unresolved_method_calls_instead_of_listing_them() {
    let lib = "pub struct A;\nimpl A {\n    pub fn get(&self) -> u8 { 1 }\n}\npub struct B;\nimpl B {\n    pub fn get(&self) -> u8 { 2 }\n}\npub fn run(v: &Vec<u8>) -> u8 {\n    v.get(0)\n}\n";
    let d = fixture(&[("lib.rs", lib)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "lib.rs#run", "--down"], "");
    assert_eq!(code, 1);
    assert_eq!(
        out,
        "lib.rs#run  fn 9-11  callees depth 2\n  ? 1 unresolved method calls: acus follow lib.rs#run --down --all\n"
    );
}

#[test]
fn a_cycle_prints_seen_above_and_stops() {
    let cycle = "pub fn a(n: u8) -> u8 {\n    b(n)\n}\npub fn b(n: u8) -> u8 {\n    a(n)\n}\n";
    let d = fixture(&[("cycle.rs", cycle)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "cycle.rs#a", "-d", "3"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "cycle.rs#a  fn 1-3  callers depth 3\n  cycle.rs#b  5  a(n)\n    cycle.rs#a  2  b(n)  (seen above)\n"
    );
}

#[test]
fn depth_limit_prints_the_next_command() {
    let d = fixture(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "lib.rs#parse", "-d", "1"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "lib.rs#parse  fn 1-3  callers depth 1\n  lib.rs#run  6  let n = parse(src);\n    … depth limit: acus follow lib.rs#run\n"
    );
}

#[test]
fn unsure_callers_collapse_past_the_cap_and_all_lifts_it() {
    let mut many = String::from("pub fn target() -> u8 {\n    0\n}\n");
    for i in 0..16 {
        many.push_str(&format!(
            "pub fn c{i}(m: &u8) -> u8 {{\n    m.target()\n}}\n"
        ));
    }
    let d = fixture(&[("many.rs", &many)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "many.rs#target"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out.lines().filter(|l| l.starts_with("  many.rs#c")).count(),
        15
    );
    assert!(
        out.lines()
            .any(|l| l == "  ? 1 more .target() calls in 1 file: acus follow many.rs#target --all"),
        "{out}"
    );
    let (_, out, _) = acus_in(d.path(), &["follow", "many.rs#target", "--all"], "");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("  many.rs#c")).count(),
        16
    );
    assert!(!out.contains("more"), "{out}");

    let (_, json, _) = acus_in(d.path(), &["follow", "many.rs#target", "--json"], "");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["truncated"],
        serde_json::json!([{"at": "many.rs#target", "more": 1, "next": "acus follow many.rs#target --all"}])
    );
}

#[test]
fn python_init_is_called_by_the_class_name() {
    let models = "class Foo:\n    def __init__(self, x):\n        self.x = x\n\n\ndef make():\n    return Foo(1)\n";
    let d = fixture(&[("models.py", models)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "models.py#Foo.__init__"], "");
    assert_eq!(code, 0);
    assert!(out.contains("callers depth 2"), "{out}");
    assert!(out.contains("models.py#make  7  return Foo(1)"), "{out}");
}

#[test]
fn typescript_constructor_is_called_by_new() {
    let models = "export class Foo {\n  constructor(x: number) {\n    this.x = x;\n  }\n}\n\nexport function make(): Foo {\n  return new Foo(1);\n}\n";
    let d = fixture(&[("models.ts", models)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "models.ts#Foo.constructor"], "");
    assert_eq!(code, 0);
    assert!(
        out.contains("models.ts#make  8  return new Foo(1);"),
        "{out}"
    );
}

#[test]
fn down_resolves_callees_in_other_files_and_drops_std_calls() {
    let down = "pub fn run() -> usize {\n    let v = Vec::new();\n    let n = helper(1);\n    println!(\"{n}\");\n    n + v.len()\n}\n";
    let helpers = "pub fn helper(x: usize) -> usize {\n    x\n}\n";
    let d = fixture(&[("down.rs", down), ("helpers.rs", helpers)]);
    let (code, out, err) = acus_in(d.path(), &["follow", "down.rs#run", "--down"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "down.rs#run  fn 1-6  callees depth 2\n  helpers.rs#helper  3  let n = helper(1);\n"
    );
}

#[test]
fn exit_codes_cover_no_callers_and_bad_addresses() {
    let d = fixture(&[
        ("lib.rs", LIB),
        ("main.rs", MAIN),
        ("notes.md", "# Title\n"),
    ]);
    // No callers: exit 1, with only the target header and no depth-limit line.
    let (code, out, _) = acus_in(d.path(), &["follow", "main.rs#main"], "");
    assert_eq!(
        (code, out.as_str()),
        (1, "main.rs#main  fn 1-4  callers depth 2\n")
    );
    let (code, _, err) = acus_in(d.path(), &["follow", "notes.md#Title"], "");
    assert_eq!(code, 2);
    assert!(err.contains("hint:"), "{err}");
    let (code, _, err) = acus_in(d.path(), &["follow", "lib.rs#Missing"], "");
    assert_eq!(code, 2);
    assert!(err.contains("no symbol `Missing`"), "{err}");
    let (code, _, _) = acus_in(d.path(), &["follow", "lib.rs:1-3"], "");
    assert_eq!(code, 2);
}

#[test]
fn every_printed_address_resolves_with_show() {
    let down = "pub fn run() -> usize {\n    let v = Vec::new();\n    let n = helper(1);\n    println!(\"{n}\");\n    n + v.len()\n}\n";
    let helpers = "pub fn helper(x: usize) -> usize {\n    x\n}\n";
    let d = fixture(&[
        ("lib.rs", LIB),
        ("main.rs", MAIN),
        ("down.rs", down),
        ("helpers.rs", helpers),
    ]);
    let runs: [&[&str]; 3] = [
        &["follow", "lib.rs#parse", "-d", "3"],
        &["follow", "lib.rs#check", "-d", "3"],
        &["follow", "down.rs#run", "--down"],
    ];
    for args in runs {
        let (_, out, _) = acus_in(d.path(), args, "");
        let addrs = printed_addrs(&out);
        assert!(!addrs.is_empty(), "{args:?}: {out}");
        for a in addrs {
            let (code, _, err) = acus_in(d.path(), &["show", &a], "");
            assert_eq!(code, 0, "{a} from {args:?}: {err}");
        }
    }
}

#[test]
fn follow_text_snapshots() {
    let _dropguard = snapshot_settings().bind_to_scope();
    let d = fixture(&[("lib.rs", LIB), ("main.rs", MAIN)]);
    insta::assert_snapshot!(
        "cli__follow_callers",
        acus_in(d.path(), &["follow", "lib.rs#parse", "-d", "3"], "").1
    );
    insta::assert_snapshot!(
        "cli__follow_down",
        acus_in(d.path(), &["follow", "lib.rs#run", "--down"], "").1
    );
}

#[test]
fn this_and_self_method_calls_are_sure_edges_both_ways() {
    let ts = "class K {\n  run() {\n    this.step();\n    helper();\n  }\n  step() {\n    helper();\n  }\n}\nfunction helper() {}\n";
    let py = "class K:\n    def run(self):\n        self.step()\n        helper()\n\n    def step(self):\n        helper()\n\n\ndef helper():\n    pass\n";
    let swift = "struct K {\n    func run() {\n        self.step()\n        helper()\n    }\n    func step() {\n        helper()\n    }\n}\nfunc helper() {}\n";
    for (file, src) in [("k.ts", ts), ("k.py", py), ("k.swift", swift)] {
        let d = fixture(&[(file, src)]);
        let step = format!("{file}#K::step");
        let run = format!("{file}#K::run");
        // The call in run is a sure caller of step.
        let (code, out, err) = acus_in(d.path(), &["follow", step.as_str()], "");
        assert_eq!((code, err.as_str()), (0, ""), "{file}");
        assert!(
            out.contains(&format!("  {run}  3  ")) && !out.contains('?'),
            "{file}: {out}"
        );
        // step is a sure callee of run, and the helper it calls is one too.
        let (code, out, err) = acus_in(d.path(), &["follow", run.as_str(), "--down"], "");
        assert_eq!((code, err.as_str()), (0, ""), "{file}");
        assert!(
            out.contains(&format!("  {step}  3  ")) && !out.contains('?'),
            "{file}: {out}"
        );
    }
}

#[test]
fn one_line_functions_are_followed_by_byte_position() {
    // Both functions sit on line 1: each call is matched to its enclosing symbol by byte.
    let rs = "fn a() { b(); } fn b() {}\n";
    let d = fixture(&[("one.rs", rs)]);
    let (code, out, err) = acus_in(d.path(), &["follow", "one.rs#b"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "one.rs#b  fn 1-1  callers depth 2\n  one.rs#a  1  fn a() { b(); } fn b() {}\n"
    );
    let (code, out, err) = acus_in(d.path(), &["follow", "one.rs#a", "--down"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "one.rs#a  fn 1-1  callees depth 2\n  one.rs#b  1  fn a() { b(); } fn b() {}\n"
    );
}

#[test]
fn one_line_recursion_is_not_a_caller_but_other_callers_are_kept() {
    // r calls itself, which is not a caller of r; s calls r on the same line and is one.
    let rs = "fn r() { r(); } fn s() { r(); }\n";
    let d = fixture(&[("rec.rs", rs)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "rec.rs#r"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "rec.rs#r  fn 1-1  callers depth 2\n  rec.rs#s  1  fn r() { r(); } fn s() { r(); }\n"
    );
    let (code, out, _) = acus_in(d.path(), &["follow", "rec.rs#s", "--down"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "rec.rs#s  fn 1-1  callees depth 2\n  rec.rs#r  1  fn r() { r(); } fn s() { r(); }\n"
    );
}

#[test]
fn minified_js_one_liner_lists_callers_and_callees() {
    let js = "function a(){b()}function b(){}\n";
    let d = fixture(&[("min.js", js)]);
    let (code, out, _) = acus_in(d.path(), &["follow", "min.js#b"], "");
    assert_eq!(code, 0);
    assert!(
        out.contains("  min.js#a  1  function a(){b()}function b(){}"),
        "{out}"
    );
    let (code, out, _) = acus_in(d.path(), &["follow", "min.js#a", "--down"], "");
    assert_eq!(code, 0);
    assert!(
        out.contains("  min.js#b  1  function a(){b()}function b(){}"),
        "{out}"
    );
}
