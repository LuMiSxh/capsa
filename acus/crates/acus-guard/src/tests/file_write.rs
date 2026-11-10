// SPDX-License-Identifier: MPL-2.0

use super::*;

fn kind(cmd: &str) -> Option<(&'static str, &'static str)> {
    check_in(cmd, false, Some(&Scope::fixed("/repo"))).map(|v| match v {
        Verdict::Deny(r, _) => ("deny", r.as_str()),
        Verdict::Hint(r, _) => ("hint", r.as_str()),
        Verdict::Escape(r) => ("escape", r.as_str()),
        Verdict::Rewrite(r) => ("rewrite", r.as_str()),
    })
}

#[test]
fn refuses_shell_writes_into_source_files() {
    for cmd in [
        "cat > src/a.rs <<'EOF'\nfn main() {}\nEOF",
        "cat >> src/a.rs <<EOF\nfn x() {}\nEOF",
        "printf 'x' > src/a.rs",
        "echo x >src/a.rs",
        "cat > src/a.rs << 'EOF'\nx\nEOF",
        "cat > src/a.rs <<- EOF\nx\nEOF",
        "cat <<< \"x\" > src/a.rs",
        "cat << EOF > src/a.rs\nx\nEOF",
        "cat > src/a.rs <<-EOF\nx\nEOF",
        "cat <<<\"x\" > src/a.rs",
        "echo x | tee src/a.rs",
        "echo x | tee -a src/a.rs",
        "cd app && cat > 'src/a.ts' <<'EOF'\nx\nEOF",
    ] {
        assert_eq!(kind(cmd), Some(("deny", "file-write")), "{cmd}");
    }
    let (_, r) = deny(check("printf 'x' > src/a.rs"));
    assert!(
        r.contains("`*** Add File: PATH`") && r.contains("Write tool"),
        "{r}"
    );
}

#[test]
fn leaves_other_writes_alone() {
    for cmd in [
        "tee -a /tmp/x.rs",
        "cat > /tmp/p.txt <<EOF\nx\nEOF",
        "cat > /tmp/a.rs <<EOF\nx\nEOF",
        "cat > $TMPDIR/a.rs <<EOF\nx\nEOF",
        "cat > notes.txt",
        "cat a.rs b.rs > /repo/all.rs",
        "cat a.rs > b.rs",
        "cat src/x.rs > src/y.rs",
        "cat < src/x.rs > src/y.rs",
        "cat > data.json <<EOF\n{}\nEOF",
        "echo x > Cargo.toml",
        "echo x > .gitignore",
        "echo x > fix.patch",
        "cargo test 2>&1 > /tmp/log",
        "echo x > /dev/null",
        "echo x >&2",
        "cargo build 2>&1",
        "echo x 2> src/a.rs",
        "cat > notes.txt <<'EOF'\n> src/a.rs\nEOF",
        "echo 'a > src/a.rs'",
    ] {
        assert_eq!(kind(cmd), None, "{cmd}");
    }
}

#[test]
fn escapes_and_downgrades_writes() {
    assert_eq!(
        kind("command cat > src/a.rs"),
        Some(("escape", "file-write"))
    );
    for cmd in [
        "for f in a b; do cat > src/$f.rs; done",
        "{ echo x > src/a.rs; }",
    ] {
        assert_eq!(kind(cmd), Some(("hint", "file-write")), "{cmd}");
    }
}

#[test]
fn refuses_edit_scripts_only_for_repo_source_files() {
    let script = |path: &str| {
        format!(
            "python3 - <<'EOF'\np = '{path}'\ns = open(p).read()\nopen(p, 'w').write(s.replace('a', 'b'))\nEOF"
        )
    };
    assert_eq!(kind(&script("src/a.rs")), Some(("deny", "edit-script")));
    assert_eq!(kind(&script("/tmp/a.rs")), Some(("hint", "edit-script")));
    assert_eq!(kind(&script(".gitattributes")), None);
    assert_eq!(
        kind("python3 -c \"open('src/a.rs','w').write(open('src/a.rs').read().replace('a','b'))\""),
        Some(("deny", "edit-script"))
    );
}
