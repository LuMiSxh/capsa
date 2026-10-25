// SPDX-License-Identifier: MPL-2.0
//! Guard tests for how a Bash line is split into segments and what that makes safe to rewrite.

use super::*;

fn segs(cmd: &str) -> Vec<Segment> {
    parse(cmd, Dialect::Bash).expect("parses").segs
}

fn rw_ast(cmd: &str) -> Option<String> {
    rewrite(cmd, None).map(|r| r.command)
}

#[test]
fn segments_know_where_their_output_goes_and_how_deep_they_sit() {
    let s = segs("{ echo; cat a.md; } > b");
    assert!(
        s.iter()
            .all(|s| s.out == Out::File && s.nest == Nest::Group)
    );

    let s = segs("cat a.md | head -3 && echo done");
    let [cat, head, echo] = &s[..] else { panic!() };
    assert_eq!((cat.out, cat.stdin, cat.gates), (Out::Pipe, false, false));
    assert_eq!((head.out, head.stdin, head.gates), (Out::Agent, true, true));
    assert_eq!(
        (echo.out, echo.nest, echo.gates),
        (Out::Agent, Nest::Top, false)
    );

    let s = segs("for f in a; do cat $f; done | wc -l");
    assert_eq!((s[0].out, s[0].nest), (Out::Pipe, Nest::Loop));
    assert_eq!(
        (s[1].out, s[1].stdin, s[1].nest),
        (Out::Agent, true, Nest::Top)
    );

    // A loop inside a group is a loop, and a function body is never the agent's output.
    let s = segs("{ while true; do cat a.md; done; }");
    assert_eq!(s[0].nest, Nest::Loop);
    let s = segs("f() { cat a.md; }");
    assert_eq!((s[0].out, s[0].nest), (Out::Capture, Nest::Group));

    // `cat a.md` gates the `||` even though the `&&` comes before it.
    let s = segs("cd X && cat a.md || echo no");
    assert_eq!(
        s.iter().map(|s| s.gates).collect::<Vec<_>>(),
        [true, true, false]
    );
}

#[test]
fn substitutions_are_segments_of_their_own_and_one_word_of_the_outer_command() {
    let s = segs("echo \"a $(cat \"b.md\") c\" | cat");
    let [echo, cat, inner] = &s[..] else { panic!() };
    assert_eq!(echo.low, "echo \"a $() c\"");
    assert_eq!(
        (inner.out, inner.low.as_str()),
        (Out::Capture, "cat \"b.md\"")
    );
    assert_eq!(cat.low, "cat");
    assert!(echo.feeds(cat));
    // A stage with a substitution does not come between the stages around it.
    let s = segs("cat $(f) | tee >(g)");
    assert_eq!(
        s.iter().map(|s| s.low.as_str()).collect::<Vec<_>>(),
        ["cat $()", "tee >", "f", "g"]
    );
}

#[test]
fn heredocs_belong_to_the_command_that_opens_them() {
    let p = parse("cat <<EOF | python\nx\nEOF\ncat a.md", Dialect::Bash).unwrap();
    let [cat, python, last] = &p.segs[..] else {
        panic!()
    };
    assert_eq!(
        (cat.low.as_str(), cat.out, cat.stdin),
        ("cat <<EOF", Out::Pipe, true)
    );
    assert_eq!((python.low.as_str(), python.stdin), ("python", true));
    assert_eq!((last.low.as_str(), last.stdin), ("cat a.md", false));
    assert_eq!(p.docs.len(), 1);
    assert_eq!(p.docs[0].body, "x\n");
    assert_eq!(p.docs[0].openers, ["cat <<EOF", "python"]);
    // The grammar hangs the heredoc of `a && b <<EOF` on the whole list; it is `b`'s.
    let s = segs("cd w && cat > f <<'EOF'\nx\nEOF");
    assert!(!s[0].heredoc && s[1].heredoc && s[1].stdin);
}

#[test]
fn a_command_in_a_heredoc_body_is_data() {
    for cmd in [
        "cat > f.md <<'EOF'\ncat a.md\nEOF",
        "cat <<'EOF'\ngrep -r foo src\nEOF",
    ] {
        assert_eq!(check(cmd), None, "{cmd}");
        assert_eq!(rw_ast(cmd), None, "{cmd}");
    }
    assert_eq!(
        rw_ast("cat <<'EOF'\ncat a.md\nEOF\ncat a.md").as_deref(),
        Some("cat <<'EOF'\ncat a.md\nEOF\nacus show 'a.md'")
    );
}

#[test]
fn rewrites_the_outer_command_next_to_a_substitution() {
    assert_eq!(
        rw_ast("x=$(cat a.md); cat a.md").as_deref(),
        Some("x=$(cat a.md); acus show 'a.md'")
    );
    assert_eq!(
        rw_ast("echo \"a $(cat \"b.md\") c\"; cat a.md").as_deref(),
        Some("echo \"a $(cat \"b.md\") c\"; acus show 'a.md'")
    );
    assert_eq!(check("echo \"a $(cat \"b.md\") c\""), None);
    assert_eq!(rw_ast("echo \"$(cat a.md)\""), None);
    // The substitution is an argument, not a path: the pipe still converts.
    assert_eq!(
        rw_ast("cargo test $(echo -q) | tail -3"),
        None,
        "an expansion in a build is not quoted into acus ctx"
    );
}

#[test]
fn keeps_what_comes_before_the_command() {
    assert_eq!(
        rw_ast("cd X && cat a.md").as_deref(),
        Some("cd X && acus show 'a.md'")
    );
    assert_eq!(
        rw_ast("FOO=1 cat a.md").as_deref(),
        Some("FOO=1 acus show 'a.md'")
    );
    assert_eq!(
        rw_ast("A=$(pwd) B=2 head -3 a.md 2>&1").as_deref(),
        Some("A=$(pwd) B=2 acus show 'a.md:1-3' 2>&1")
    );
    assert_eq!(
        rw_ast("cd X && cargo build 2>&1 | grep error").as_deref(),
        Some("cd X && acus ctx 'cargo build' --grep 'error'")
    );
}

#[cfg(feature = "bash")]
#[test]
fn zsh_syntax_the_bash_grammar_rejects_runs_unchanged() {
    for cmd in [
        "print -rl -- ${(f)\"$(cat a.md)\"}",
        "cat a.md ${(f)x}",
        "cat a.md | grep 'x",
    ] {
        assert!(
            parse_with(cmd, Dialect::Bash, Backend::Bash).is_none(),
            "{cmd}"
        );
        assert_eq!(check(cmd), None, "{cmd}");
        assert_eq!(rw_ast(cmd), None, "{cmd}");
    }
}

#[test]
fn absurd_nesting_runs_unchanged_instead_of_overflowing() {
    let deep = format!("{}cat a.md{}", "(".repeat(5000), ")".repeat(5000));
    assert_eq!(check(&deep), None);
    assert_eq!(rw_ast(&deep), None);
}

#[test]
fn a_heredoc_pipe_in_a_list_feeds_only_the_command_before_the_next_gate() {
    let s = segs("cd X && acus patch <<'P' | tail -3 && echo done\nbody\nP\n");
    let [_, patch, tail, echo] = s.as_slice() else {
        panic!("four segments expected");
    };
    assert_eq!(
        (patch.out, tail.stdin, echo.stdin),
        (Out::Pipe, true, false)
    );
}
