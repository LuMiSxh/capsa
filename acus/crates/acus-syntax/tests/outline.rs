// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

use acus_syntax::{KINDS, Lang, outline};

fn rows(lang: Lang, src: &str) -> Vec<String> {
    outline(lang, src)
        .unwrap()
        .symbols
        .iter()
        .map(|s| {
            format!(
                "{}{} {} {}-{}",
                "  ".repeat(s.depth),
                s.kind,
                s.qual,
                s.start_line,
                s.end_line
            )
        })
        .collect()
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_outline() {
    let src = "mod m {\n    pub struct A;\n    impl Display for A<T> {\n        fn fmt(&self) {}\n    }\n}\nfn main() {\n    fn inner() {}\n}\n";
    assert_eq!(
        rows(Lang::Rust, src),
        [
            "mod m 1-6",
            "  struct m::A 2-2",
            "  impl m::A 3-5",
            "    fn m::A::fmt 4-4",
            "fn main 7-9",
        ]
    );
    let o = outline(Lang::Rust, src).unwrap();
    assert_eq!(o.symbols[2].detail.as_deref(), Some("Display for A<T>"));
    assert_eq!(o.enclosing(4).unwrap().qual, "m::A::fmt");
    // Function bodies are leaves, so line 8 belongs to `main`.
    assert_eq!(o.enclosing(8).unwrap().qual, "main");
    assert!(o.enclosing(10).is_none());
    assert_eq!(o.lines, 9);
}

#[test]
fn lang_from_path() {
    use std::path::Path;
    assert_eq!(Lang::from_path(Path::new("a/b.RS")), Some(Lang::Rust));
    assert_eq!(Lang::from_path(Path::new("x.yml")), Some(Lang::Yaml));
    assert_eq!(Lang::from_path(Path::new("x.cpp")), None);
    assert_eq!(Lang::from_path(Path::new("run.BASH")), Some(Lang::Shell));
    assert_eq!(Lang::from_path(Path::new("a/api.h")), Some(Lang::C));
    assert_eq!(
        Lang::from_path(Path::new("a/Program.cs")),
        Some(Lang::CSharp)
    );
    assert_eq!(Lang::from_path(Path::new("a/main.go")), Some(Lang::Go));
}

/// Returns the source and expected rows of each language whose outline is pinned here.
#[expect(
    clippy::too_many_lines,
    clippy::vec_init_then_push,
    reason = "one table of per-language outlines, each row behind its Cargo feature"
)]
fn language_outline_cases() -> Vec<(Lang, &'static str, Vec<&'static str>)> {
    let mut cases = Vec::new();
    #[cfg(feature = "lang-swift")]
    cases.push((
        Lang::Swift,
        "public struct S {\n  var p: Int = 1\n  init() {}\n  func f() {}\n}\nextension S: P {\n  func g() {}\n}\nprotocol P {\n  func h()\n}\nenum E { case a }\ntypealias T = Int\n",
        vec![
            "struct S 1-5",
            "  var S::p 2-2",
            "  func S::init 3-3",
            "  func S::f 4-4",
            "extension S 6-8",
            "  func S::g 7-7",
            "protocol P 9-11",
            "  func P::h 10-10",
            "enum E 12-12",
            "typealias T 13-13",
        ],
    ));
    #[cfg(feature = "lang-ts")]
    cases.push((
        Lang::TypeScript,
        "export class A {\n  m() {}\n}\ninterface I { x: number }\ntype T = string\nenum E { A }\nexport const f = () => 1\nconst n = 1\nfunction g() {\n  const inner = () => 2\n}\n",
        vec![
            "class A 1-3",
            "  method A::m 2-2",
            "interface I 4-4",
            "type T 5-5",
            "enum E 6-6",
            "function f 7-7",
            "function g 9-11",
        ],
    ));
    #[cfg(feature = "lang-python")]
    cases.push((
        Lang::Python,
        "class A:\n    def b(self):\n        pass\n\n@dec\ndef c():\n    pass\n",
        vec!["class A 1-3", "  def A::b 2-3", "def c 6-7"],
    ));
    #[cfg(feature = "lang-svelte")]
    cases.push((
        Lang::Svelte,
        "<script lang=\"ts\">\n  function f() {}\n  const g = () => 1\n</script>\n\n<p>hi</p>\n",
        vec!["function f 2-2", "function g 3-3"],
    ));
    #[cfg(all(feature = "lang-scss", not(windows)))]
    cases.push((
        Lang::Scss,
        "a {\n  b { color: red; }\n}\n",
        vec!["rule a 1-3", "  rule a::b 2-2"],
    ));
    #[cfg(feature = "lang-css")]
    // A custom property counts only in `:root`; `--pad` in `h2` is not listed.
    cases.push((
        Lang::Css,
        ":root {\n  --main: #333;\n  color: red;\n}\nh1,\nh2 { margin: 0; --pad: 2px }\n/* note */\n@media (max-width: 600px) {\n  .nav a { color: blue; }\n}\n@supports (display: grid) { .grid { display: grid; } }\n@keyframes spin { from { transform: rotate(0); } }\n",
        vec![
            "rule :root 1-4",
            "  var :root::--main 2-2",
            "rule h1, h2 5-6",
            "media (max-width: 600px) 8-10",
            "  rule (max-width: 600px)::.nav a 9-9",
            "supports (display: grid) 11-11",
            "  rule (display: grid)::.grid 11-11",
            "keyframes spin 12-12",
        ],
    ));
    #[cfg(feature = "lang-html")]
    // Plain `div`s and `html`/`head`/`body` are not listed; their children still are.
    cases.push((
        Lang::Html,
        "<!DOCTYPE html>\n<!-- page shell -->\n<html>\n<head>\n<style>body { margin: 0 }</style>\n<script>var x = 1;</script>\n</head>\n<body>\n<div id=\"main\"><p>hi</p><div class=\"x\"></div></div>\n<section id=\"hero\"><h1>T</h1></section>\n<nav><a href=\"#\">a</a></nav>\n<template id=\"row\"><span></span></template>\n</body>\n</html>\n",
        vec![
            "style style 5-5",
            "script script 6-6",
            "element div#main 9-9",
            "section section#hero 10-10",
            "nav nav 11-11",
            "template template#row 12-12",
        ],
    ));
    #[cfg(feature = "lang-c")]
    cases.push((
        Lang::C,
        "#define MAX 10\n#define ADD(a, b) ((a) + (b))\nstruct S {\n  int x;\n};\nstruct Fwd;\nunion U { int i; };\nenum E { A, B };\ntypedef struct { int x; } T;\ntypedef int *P;\n// adds\nstatic int add(int a, int b) {\n  return a + b;\n}\nint *p(void);\nint v;\n",
        vec![
            "macro MAX 1-1",
            "macro ADD 2-2",
            "struct S 3-5",
            "union U 7-7",
            "enum E 8-8",
            "typedef T 9-9",
            "typedef P 10-10",
            "fn add 12-14",
            "fn p 15-15",
        ],
    ));
    #[cfg(feature = "lang-csharp")]
    cases.push((
        Lang::CSharp,
        "using System;\nnamespace App {\n    /// A repository.\n    public class Repo {\n        public Repo() {}\n        public int Count { get; set; }\n        private string name = \"\";\n        public void Load() {}\n    }\n    public interface IRepo {\n        void Load();\n    }\n    public struct Point { public int X; }\n    public record Pair(int A, int B);\n    public enum Color { Red, Blue }\n    public delegate void Handler(object sender);\n}\n",
        vec![
            "namespace App 2-17",
            "  class App::Repo 4-9",
            "    constructor App::Repo::Repo 5-5",
            "    property App::Repo::Count 6-6",
            "    field App::Repo::name 7-7",
            "    method App::Repo::Load 8-8",
            "  interface App::IRepo 10-12",
            "    method App::IRepo::Load 11-11",
            "  struct App::Point 13-13",
            "    field App::Point::X 13-13",
            "  record App::Pair 14-14",
            "  enum App::Color 15-15",
            "  delegate App::Handler 16-16",
        ],
    ));
    #[cfg(feature = "lang-go")]
    cases.push((
        Lang::Go,
        "package main\n\nimport \"fmt\"\n\nconst Version = \"1\"\n\nvar counter int\n\ntype Point struct {\n\tX int\n}\n\ntype Shape interface {\n\tArea() float64\n}\n\ntype ID = int\n\nfunc main() {\n\tfmt.Println(Version)\n\tvar local int\n\tconst inner = 1\n\t_ = local + inner\n}\n\nfunc (p Point) Move() {}\n\nfunc add(a, b int) int { return a + b }\n",
        vec![
            "namespace main 1-1",
            "const Version 5-5",
            "var counter 7-7",
            "struct Point 9-11",
            "interface Shape 13-15",
            "typealias ID 17-17",
            "func main 19-24",
            "method Move 26-26",
            "func add 28-28",
        ],
    ));
    cases
}

#[test]
fn language_outlines() {
    for (lang, src, want) in language_outline_cases() {
        assert_eq!(rows(lang, src), want, "{}", lang.name());
    }
}

#[cfg(feature = "lang-markdown")]
#[test]
fn markdown_outline() {
    let src = "# Title\n\ntext\n\n## Sub One\n\nx\n\n# Two\n";
    assert_eq!(
        rows(Lang::Markdown, src),
        ["h1 Title 1-8", "  h2 Title::Sub One 5-8", "h1 Two 9-9"]
    );
    let setext = "# T\n\nFirst\n-----\n\na\n\nSecond\n------\nb\n";
    assert_eq!(
        rows(Lang::Markdown, setext),
        ["h1 T 1-10", "  h2 T::First 3-7", "  h2 T::Second 8-10"]
    );
}

#[cfg(feature = "lang-data")]
#[test]
fn data_outlines_nest_keys() {
    assert_eq!(
        rows(Lang::Toml, "[a]\nb = 1\n[[c.d]]\ne = 2\n"),
        [
            "table a 1-2",
            "  key a::b 2-2",
            "array c.d 3-4",
            "  key c.d::e 4-4"
        ]
    );
    assert_eq!(
        rows(Lang::Json, "{\n  \"a\": {\"x\": 1},\n  \"b\": 2\n}\n"),
        ["key a 2-2", "  key a::x 2-2", "key b 3-3"]
    );
    assert_eq!(
        rows(Lang::Yaml, "a:\n  x: 1\nb: 2\n"),
        ["key a 1-2", "  key a::x 2-2", "key b 3-3"]
    );
}

#[cfg(feature = "lang-shell")]
#[test]
fn shell_outline() {
    let src = "# greets\ngreet() { echo hi; }\n\nfunction build {\n  echo b\n}\n";
    assert_eq!(rows(Lang::Shell, src), ["fn greet 2-2", "fn build 4-6"]);
    // The `#` comment above `greet` belongs to it.
    let o = outline(Lang::Shell, src)
        .unwrap()
        .with_attached(src, "run.sh");
    assert_eq!(o.symbols[0].start_line, 1);
    assert_eq!(o.symbols[1].start_line, 4);
}

#[cfg(feature = "lang-xml")]
#[test]
fn xml_outline_and_plist_keys() {
    // Plist: a `<key>` is named by its text, and the value after it is not listed.
    let plist = "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n  <!-- bundle -->\n  <key>CFBundleIdentifier</key>\n  <string>com.x.y</string>\n  <key>Nested</key>\n  <dict><key>deep</key><string>v</string></dict>\n</dict>\n</plist>\n";
    assert_eq!(
        rows(Lang::Xml, plist),
        [
            "element plist 2-10",
            "  element plist::dict 3-9",
            "    key plist::dict::CFBundleIdentifier 5-5",
            "    key plist::dict::Nested 7-7",
        ]
    );
    // Elements stop at depth 2, and `id`, `name` or `key` attributes name them.
    let src = "<root><item id=\"a\"><sub/></item><item name=\"b\"/><a><b><c><d id=\"x\"/></c></b></a></root>\n";
    assert_eq!(
        rows(Lang::Xml, src),
        [
            "element root 1-1",
            "  element root::item#a 1-1",
            "    element root::item#a::sub 1-1",
            "  element root::item#b 1-1",
            "  element root::a 1-1",
            "    element root::a::b 1-1",
        ]
    );
}

fn assert_listed(lang: Lang, src: &str) {
    for s in outline(lang, src).unwrap().symbols {
        assert!(
            KINDS.contains(&s.kind),
            "{} emits `{}`, which KINDS does not list",
            lang.name(),
            s.kind
        );
    }
}

#[test]
fn every_outline_kind_is_listed() {
    #[cfg(feature = "lang-rust")]
    assert_listed(
        Lang::Rust,
        "mod m { pub struct A; impl Display for A<T> { fn fmt(&self) {} } }\nfn main() {}\nunion U { a: u8 }\nenum E { A }\nconst C: u8 = 1;\nstatic S: u8 = 1;\ntype T = u8;\nmacro_rules! m { () => {} }\ntrait Tr { fn f(); }\n",
    );
    #[cfg(feature = "lang-swift")]
    assert_listed(
        Lang::Swift,
        "public struct S {\n  var p: Int = 1\n  init() {}\n  func f() {}\n}\nextension S: P {\n  func g() {}\n}\nprotocol P {\n  func h()\n}\nenum E { case a }\ntypealias T = Int\nactor A {}\nclass C {}\n",
    );
    #[cfg(feature = "lang-ts")]
    assert_listed(
        Lang::TypeScript,
        "class C { m() {} }\nabstract class D {}\nnamespace N {}\nfunction f() {}\nfunction* g() {}\ninterface I {}\ntype T = number;\nenum E { A }\nconst h = () => 1;\n",
    );
    #[cfg(feature = "lang-python")]
    assert_listed(
        Lang::Python,
        "class C:\n    def m(self):\n        pass\n\ndef f():\n    pass\n",
    );
    #[cfg(feature = "lang-markdown")]
    assert_listed(
        Lang::Markdown,
        "# A\n\n## B\n\n### C\n\n#### D\n\n##### E\n\n###### F\n\nSetext\n------\n",
    );
    #[cfg(feature = "lang-svelte")]
    assert_listed(
        Lang::Svelte,
        "<script lang=\"ts\">\n  function f() {}\n</script>\n",
    );
    #[cfg(feature = "lang-data")]
    assert_listed(Lang::Toml, "[t]\na = 1\n[[arr]]\nb = 2\n");
    #[cfg(feature = "lang-data")]
    assert_listed(Lang::Json, "{\"k\": {\"s\": 1}}");
    #[cfg(feature = "lang-data")]
    assert_listed(Lang::Yaml, "k:\n  sub: 1\n");
    #[cfg(feature = "lang-shell")]
    assert_listed(Lang::Shell, "f() { echo; }\n");
    #[cfg(feature = "lang-c")]
    assert_listed(
        Lang::C,
        "struct S { int a; };\nunion U { int a; };\nenum E { A };\ntypedef int T;\n#define M 1\n#define F(x) x\nint g(void);\nint f(void) { return 0; }\n",
    );
    #[cfg(feature = "lang-csharp")]
    assert_listed(
        Lang::CSharp,
        "namespace N {\n    class C { }\n    interface I { }\n    struct S { }\n    enum E { A }\n    record R;\n    delegate void D();\n    class M {\n        M() {}\n        int P { get; }\n        int f;\n        event System.Action Ev;\n        void Run() {}\n    }\n}\n",
    );
    #[cfg(feature = "lang-go")]
    assert_listed(
        Lang::Go,
        "package main\nconst C = 1\nvar V int\ntype S struct{}\ntype I interface{}\ntype T int\ntype A = int\nfunc f() {}\nfunc (s S) M() {}\n",
    );
    #[cfg(feature = "lang-css")]
    assert_listed(
        Lang::Css,
        ".a { color: red; }\n@media (min-width: 1px) { .b { x: y; } }\n@supports (display: grid) { .c { x: y; } }\n@keyframes spin { from { x: y; } }\n:root { --x: 1; }\n",
    );
    #[cfg(feature = "lang-html")]
    assert_listed(
        Lang::Html,
        "<main><section id=\"s\"><nav></nav></section></main>\n<header></header>\n<footer></footer>\n<template></template>\n<script>let x;</script>\n<style>p {}</style>\n<div id=\"d\"></div>\n",
    );
    #[cfg(feature = "lang-xml")]
    assert_listed(
        Lang::Xml,
        "<root><item id=\"a\"><key>k</key><string>v</string></item></root>\n",
    );
}
