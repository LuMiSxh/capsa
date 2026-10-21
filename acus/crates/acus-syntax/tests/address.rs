// SPDX-License-Identifier: MPL-2.0
use acus_syntax::{Address, Target};

#[test]
fn parses_all_forms() {
    assert_eq!(
        Address::parse("src/a.rs#A::t").target,
        Target::Symbol("A::t".into())
    );
    assert_eq!(
        Address::parse("src/a.rs:10-20").target,
        Target::Lines(10, 20)
    );
    assert_eq!(Address::parse("src/a.rs:7").target, Target::Lines(7, 7));
    assert_eq!(Address::parse("src/a.rs:-7").target, Target::Tail(7));
    assert_eq!(Address::parse("src/a.rs:-0").target, Target::Tail(0));
    assert_eq!(Address::parse("src/a.rs:3-").target, Target::From(3));
    assert_eq!(Address::parse("src/a.rs:0-").target, Target::From(0));
    assert_eq!(Address::parse("my-dir/a-b.rs:3-").path, "my-dir/a-b.rs");
    assert_eq!(Address::parse("src/a.rs:-").target, Target::Whole);
    assert_eq!(Address::parse("src/a.rs:3-x").target, Target::Whole);
    let win_from = Address::parse(r"C:\x\a.rs:3-");
    assert_eq!(
        (win_from.path.as_str(), win_from.target),
        (r"C:\x\a.rs", Target::From(3))
    );
    assert_eq!(Address::parse("src/a.rs:-x").target, Target::Whole);
    assert_eq!(
        Address::parse("src/a.rs:10,20").target,
        Target::Lines(10, 20)
    );
    assert_eq!(Address::parse("src/a.rs").target, Target::Whole);
    let win = Address::parse(r"C:\x\a.rs:3");
    assert_eq!(
        (win.path.as_str(), win.target),
        (r"C:\x\a.rs", Target::Lines(3, 3))
    );
    assert_eq!(Address::parse(r"C:\x\a.rs").target, Target::Whole);
    assert_eq!(
        Address::parse("README.md#Sub One").target,
        Target::Symbol("Sub One".into())
    );
    for s in [
        "src/a.rs#A::t",
        "src/a.rs:10-20",
        "src/a.rs:7",
        "src/a.rs:-7",
        "src/a.rs:3-",
        "src/a.rs",
    ] {
        assert_eq!(Address::parse(s).to_string(), s);
    }
}

#[cfg(feature = "lang-rust")]
#[test]
fn resolves_exact_suffix_and_ambiguity() {
    use acus_syntax::{Lang, Resolve, outline, resolve};
    let src = "struct A;\nimpl A {\n    fn new() {}\n}\nstruct B;\nimpl B {\n    fn new() {}\n}\nfn only() {}\n";
    let o = outline(Lang::Rust, src).unwrap();
    assert!(matches!(resolve(&o, "A::new"), Resolve::Found(s) if s.start_line == 3));
    assert!(matches!(resolve(&o, "only"), Resolve::Found(s) if s.qual == "only"));
    match resolve(&o, "new") {
        Resolve::Ambiguous(v) => assert_eq!(
            v.iter().map(|s| s.qual.as_str()).collect::<Vec<_>>(),
            ["A::new", "B::new"]
        ),
        _ => panic!("expected ambiguity"),
    }
    assert!(matches!(resolve(&o, "nope"), Resolve::Missing));
    // `A` names both the struct and its impl: prefer the container that has children.
    assert!(matches!(resolve(&o, "A"), Resolve::Found(s) if s.kind == "impl"));
}

#[cfg(feature = "lang-markdown")]
#[test]
fn twin_headings_are_ambiguous() {
    use acus_syntax::{Lang, Resolve, outline, resolve};
    let o = outline(Lang::Markdown, "# T\n## Usage\na\n## Usage\nb\n").unwrap();
    assert!(matches!(resolve(&o, "Usage"), Resolve::Ambiguous(v) if v.len() == 2));
}

#[cfg(feature = "lang-shell")]
#[test]
fn shell_function_address() {
    use acus_syntax::{Lang, Resolve, outline, resolve};
    let src = "# greets\ngreet() { echo hi; }\n\nfunction build {\n  echo b\n}\n";
    // `file.sh#build` names a function; `greet` starts at its attached comment.
    assert_eq!(
        Address::parse("file.sh#build").target,
        Target::Symbol("build".into())
    );
    let o = outline(Lang::Shell, src)
        .unwrap()
        .with_attached(src, "file.sh");
    assert!(
        matches!(resolve(&o, "build"), Resolve::Found(s) if s.kind == "fn" && s.start_line == 4)
    );
    assert!(matches!(resolve(&o, "greet"), Resolve::Found(s) if s.start_line == 1));
}

#[cfg(feature = "lang-html")]
#[test]
fn html_id_address() {
    use acus_syntax::{Lang, Resolve, outline, resolve};
    // `page.html#div#main` names the element by its tag and id.
    assert_eq!(
        Address::parse("page.html#div#main").target,
        Target::Symbol("div#main".into())
    );
    let src = "<body>\n<div id=\"main\">\n<p>x</p>\n</div>\n</body>\n";
    let o = outline(Lang::Html, src).unwrap();
    assert!(matches!(resolve(&o, "div#main"), Resolve::Found(s) if s.start_line == 2));
}

#[cfg(feature = "lang-xml")]
#[test]
fn plist_key_address() {
    use acus_syntax::{Lang, Resolve, outline, resolve};
    // The comment above a key moves with it, as in other languages.
    let src = "<plist>\n<dict>\n  <!-- id -->\n  <key>CFBundleIdentifier</key>\n  <string>x</string>\n</dict>\n</plist>\n";
    let o = outline(Lang::Xml, src)
        .unwrap()
        .with_attached(src, "Info.plist");
    assert!(
        matches!(resolve(&o, "CFBundleIdentifier"), Resolve::Found(s) if s.kind == "key" && s.start_line == 3)
    );
}
