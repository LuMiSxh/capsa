// SPDX-License-Identifier: MPL-2.0

use super::pipelines::pipelines;
use super::shape::shape_pipe;
use super::*;

fn categories(cmd: &str) -> Vec<&'static str> {
    classify_shaped(cmd)
        .into_iter()
        .map(|(category, _)| category)
        .collect()
}

fn shape(cmd: &str) -> String {
    pipelines(cmd)
        .iter()
        .map(|pipe| shape_pipe(pipe))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

#[test]
fn classifies() {
    let t: &[(&str, &[&str])] = &[
        ("acus ctx 'cargo test'", &["acus:ctx"]),
        ("acus find 'a|b' --block", &["acus:find"]),
        ("acus patch <<'EOF'\ngrep x\nEOF", &["acus:patch"]),
        ("cargo test 2>&1 | tail -20", &["build-filtered"]),
        ("cargo build --release", &["build-unfiltered"]),
        ("cd x && npm run build | grep error", &["build-filtered"]),
        ("go test ./...", &["build-unfiltered"]),
        ("sed -i '' s/a/b/ f", &["edit-sed"]),
        ("perl -pi -e 's/a/b/' f", &["edit-sed"]),
        ("sed -n '1,5p' f", &["read"]),
        ("grep -rn x . | head", &["search"]),
        (
            "find . -name '*.rs' | xargs sed -i s/a/b/",
            &["search", "edit-sed"],
        ),
        ("cat a.rs | grep x", &["read"]),
        (
            "python3 - <<'PY'\nimport pathlib\np=pathlib.Path('a')\np.write_text(p.read_text().replace('a','b'))\nPY",
            &["edit-python"],
        ),
        (
            "python3 -c \"import re;print(re.sub('a','b','a'))\"",
            &["edit-python"],
        ),
        ("python3 script.py", &["python-other"]),
        ("git diff HEAD~1 | head", &["diff"]),
        ("git -C x show abc", &["diff"]),
        (
            "apply_patch <<'EOF'\n*** Begin Patch\nEOF",
            &["edit-builtin"],
        ),
        ("cat > notes.md <<'EOF'\nhi\nEOF", &["edit-shell"]),
        ("cat >> log.txt", &["edit-shell"]),
        ("cat <<'EOF' > f.txt\nx\nEOF", &["edit-shell"]),
        ("tee out.txt <<'EOF'\nx\nEOF", &["edit-shell"]),
        ("printf 'a\\n' > f", &["edit-shell"]),
        ("cat a.rs 2>&1 | head", &["read"]),
        ("cat a.rs > /dev/null", &["read"]),
        ("ls", &[]),
    ];
    for (cmd, want) in t {
        assert_eq!(&categories(cmd), want, "{cmd}");
    }
}

#[test]
fn shapes_follow_the_segment_that_triggered_the_category() {
    let t: &[(&str, &str, &str)] = &[
        (
            "acus patch -q <<'P'\n*** Begin Patch\nP\n&& cargo test",
            "build-unfiltered",
            "cargo test",
        ),
        ("cd x && sed -n 1,5p f", "read", "sed -n"),
        ("cat > f <<'EOF'\nhi\nEOF", "edit-shell", "cat >"),
        ("python3 - <<'PY'", "python-other", "python3 -"),
    ];
    for &(cmd, category, want) in t {
        let hits = classify_shaped(cmd);
        let shape = hits
            .iter()
            .find(|(c, _)| *c == category)
            .and_then(|(_, s)| s.as_deref());
        assert_eq!(shape, Some(want), "{cmd}");
    }
}

#[test]
fn codex_arguments_are_decoded() {
    assert_eq!(
        codex_commands(r#""{cmd: \"acus find x --block\"}""#),
        ["acus find x --block"]
    );
    assert_eq!(
        codex_commands(r#""{\"cmd\":\"acus show a.rs:1-5\"}""#),
        ["acus show a.rs:1-5"]
    );
    assert_eq!(
        codex_commands(r#"{"command":["bash","-lc","cargo test"]}"#),
        ["cargo test"]
    );
}

#[test]
fn builtin_tools() {
    assert_eq!(claude_tool("Read", "").categories, ["read"]);
    assert_eq!(claude_tool("Glob", "").categories, ["search"]);
    assert_eq!(claude_tool("MultiEdit", "").categories, ["edit-builtin"]);
    assert_eq!(
        claude_tool("Bash", r#"{"command":"rg x"}"#).categories,
        ["search"]
    );
    assert_eq!(
        codex_tool("shell", r#""{\"cmd\":\"sed -n 1p a\"}""#).categories,
        ["read"]
    );
    assert_eq!(
        codex_tool("shell", r#"{"command":["bash","-lc","git diff"]}"#).categories,
        ["diff"]
    );
    assert_eq!(
        codex_tool("apply_patch", "*** Begin Patch").categories,
        ["edit-builtin"]
    );
}

#[test]
fn shapes_group_equal_commands() {
    let t: &[(&str, &str)] = &[
        ("git diff --cached", "git diff --cached"),
        ("git diff HEAD~1 src/a.rs", "git diff"),
        ("FOO=1 cargo test", "cargo test"),
        ("cargo test 2>&1 | tail -20", "cargo test | tail"),
        ("cd x && git diff", "git diff"),
        ("cd x && npm run build | grep error", "npm run | grep"),
        ("sed -n '1,5p' a.rs", "sed -n"),
        ("cat a.rs | grep x", "cat | grep"),
        ("grep -rn 'foo bar' src | head", "grep -rn | head"),
        ("python3 script.py", "python3"),
        ("cat a.rs", "cat"),
        ("cat > f <<'EOF'\nx\nEOF", "cat >"),
        ("cat <<'EOF' > f\nx\nEOF", "cat >"),
        ("ls", "ls"),
        ("cd x", ""),
    ];
    for (cmd, want) in t {
        assert_eq!(shape(cmd), *want, "{cmd}");
    }
}
