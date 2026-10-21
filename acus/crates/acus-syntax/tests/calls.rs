// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

use acus_syntax::Lang;
use acus_syntax::calls::{analyze, supports_calls};

/// One row per call: `name qual form line`, with ` macro` when inside a Rust macro.
/// `-` stands for no qualifier.
fn rows(lang: Lang, src: &str) -> Vec<String> {
    analyze(lang, src)
        .unwrap()
        .1
        .iter()
        .map(|c| {
            format!(
                "{} {} {:?} {}{}",
                c.name,
                c.qual.as_deref().unwrap_or("-"),
                c.form,
                c.line,
                if c.in_macro { " macro" } else { "" }
            )
        })
        .collect()
}

#[test]
fn support_matches_languages_with_rules() {
    assert!(supports_calls(Lang::Rust));
    assert!(supports_calls(Lang::Tsx));
    assert!(!supports_calls(Lang::Json));
    assert!(!supports_calls(Lang::Svelte));
}

#[test]
fn unsupported_language_keeps_outline_without_calls() {
    let (outline, calls) = analyze(Lang::Json, "{\"a\": {\"b\": 1}}").unwrap();
    assert!(calls.is_empty());
    assert_eq!(outline.lines, 1);
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_calls_and_negatives() {
    let src = "fn main() {\n    // parse(comment)\n    let s = \"parse(string)\";\n    let a = parse(x);\n    let b = crate::p::Parser::parse(x);\n    let c = self.parse();\n    let d = Self::make();\n    let e = x.parse();\n    let f = foo::<u8>(x);\n    let g = Foo { a: 1 };\n    let k = parse;\n    assert_eq!(parse(x), 1);\n    vec!(Parser::new(), self.run());\n    println!(\"{}\", x);\n    m!(a.b.c(1));\n}\nfn parse(x: u8) {}\nuse crate::parse::Parser;\n";
    assert_eq!(
        rows(Lang::Rust, src),
        [
            "parse - Bare 4",
            "parse Parser Path 5",
            "parse self SelfRecv 6",
            "make Self SelfRecv 7",
            "parse x Method 8",
            "foo - Bare 9",
            "Foo - Ctor 10",
            "assert_eq - Macro 12",
            "parse - Bare 12 macro",
            "vec - Macro 13",
            "new Parser Path 13 macro",
            "run self SelfRecv 13 macro",
            "println - Macro 14",
            "m - Macro 15",
            "c - Method 15 macro",
        ]
    );
}

#[cfg(feature = "lang-python")]
#[test]
fn python_calls_and_negatives() {
    let src = "import os\nfrom a import b\nclass Repo:\n    def load(self):\n        # parse(comment)\n        s = \"parse(x)\"\n        x = parse(s)\n        self.save(x)\n        cls.make()\n        obj.run()\n        return Foo(x)\n    @trace\n    def save(self, x):\n        pass\n@app.route(\"/\")\ndef index():\n    pass\n";
    assert_eq!(
        rows(Lang::Python, src),
        [
            "parse - Bare 7",
            "save self SelfRecv 8",
            "make cls SelfRecv 9",
            "run obj Method 10",
            "Foo - Bare 11",
            "trace - Bare 12",
            "route app Method 15",
        ]
    );
}

#[cfg(feature = "lang-ts")]
#[test]
fn typescript_calls_this_and_new() {
    let src = "import { parse } from \"./p\";\nclass Repo {\n  // parse(comment)\n  load(x: string) {\n    const s = \"parse(x)\";\n    const a = parse(x);\n    this.save(a);\n    obj.run();\n    const n = new Foo(a);\n    const m = new ns.Bar();\n    return this.make().chain();\n  }\n  save(x: string) {}\n}\nconst p = parse;\n";
    assert_eq!(
        rows(Lang::TypeScript, src),
        [
            "parse - Bare 6",
            "save this SelfRecv 7",
            "run obj Method 8",
            "Foo - Ctor 9",
            "Bar ns Ctor 10",
            "chain - Method 11",
            "make this SelfRecv 11",
        ]
    );
}

#[cfg(feature = "lang-ts")]
#[test]
fn tsx_jsx_elements_are_constructors() {
    let src = "const v = (\n  <Layout>\n    <Card title=\"x\" />\n    <p>{parse(x)}</p>\n  </Layout>\n);\n";
    assert_eq!(
        rows(Lang::Tsx, src),
        ["Layout - Ctor 2", "Card - Ctor 3", "parse - Bare 4"]
    );
}

#[cfg(feature = "lang-ts")]
#[test]
fn tsx_this_method_calls_are_self_recv() {
    let src = "class Panel {\n  render() {\n    return this.body();\n  }\n  body() {\n    return 1;\n  }\n}\n";
    assert_eq!(rows(Lang::Tsx, src), ["body this SelfRecv 3"]);
}

#[cfg(feature = "lang-ts")]
#[test]
fn javascript_calls_this_and_new() {
    let src = "function run() {\n  const a = helper(1);\n  this.start();\n  const r = new Thing();\n  return a;\n}\n";
    assert_eq!(
        rows(Lang::JavaScript, src),
        ["helper - Bare 2", "start this SelfRecv 3", "Thing - Ctor 4"]
    );
}

#[cfg(feature = "lang-c")]
#[test]
fn c_calls_and_negatives() {
    let src = "#include <stdio.h>\n/* parse(comment) */\nstatic int run(void) {\n    char *s = \"parse(x)\";\n    int a = parse(x);\n    obj->send(a);\n    list.push(a);\n    return helper();\n}\nint parse(int x) { return x; }\n";
    assert_eq!(
        rows(Lang::C, src),
        [
            "parse - Bare 5",
            "send obj Method 6",
            "push list Method 7",
            "helper - Bare 8",
        ]
    );
}

#[cfg(feature = "lang-swift")]
#[test]
fn swift_calls_and_self() {
    let src = "import Foundation\n// parse(comment)\nstruct Repo {\n    func load(_ x: String) {\n        let s = \"parse(x)\"\n        let a = parse(x)\n        self.save(a)\n        obj.run()\n        let r = Repo()\n    }\n    func save(_ x: String) {}\n}\n";
    assert_eq!(
        rows(Lang::Swift, src),
        [
            "parse - Bare 6",
            "save self SelfRecv 7",
            "run obj Method 8",
            "Repo - Bare 9",
        ]
    );
}

#[cfg(feature = "lang-shell")]
#[test]
fn shell_commands_are_bare_calls() {
    let src = "#!/bin/sh\n# parse x\nhelper() {\n  echo \"parse\"\n  git status\n  parse \"$1\"\n}\nhelper\n";
    assert_eq!(
        rows(Lang::Shell, src),
        [
            "echo - Bare 4",
            "git - Bare 5",
            "parse - Bare 6",
            "helper - Bare 8",
        ]
    );
}
