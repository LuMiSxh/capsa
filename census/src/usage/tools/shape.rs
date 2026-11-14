// SPDX-License-Identifier: MPL-2.0
//! Renders the canonical shape of a pipeline and reads the programs and flags of a stage.

pub(super) fn base(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

/// Programs whose first non-flag argument is a subcommand rather than a path.
const SUBCOMMANDS: [&str; 11] = [
    "acus", "bun", "cargo", "docker", "gh", "git", "go", "kubectl", "npm", "pnpm", "yarn",
];

/// Flags whose value is the next word, so the subcommand search skips it.
const VALUE_FLAGS: [&str; 5] = ["-C", "-c", "--git-dir", "--namespace", "--work-tree"];

/// The shape of one pipeline: the first real program with its subcommand and flags, the name of
/// each pipe target, and ` >` when it redirects to a file. Paths, quoted strings and other
/// arguments drop out, so equal commands group (`git diff --cached`, `cargo test | tail`,
/// `cd x && sed -n`).
pub(super) fn shape_pipe(pipe: &[Vec<String>]) -> String {
    if pipe
        .first()
        .and_then(|stage| program(stage).first())
        .is_none_or(|w| base(w) == "cd")
    {
        return String::new();
    }
    let mut out = String::new();
    for (n, stage) in pipe.iter().enumerate() {
        let mut p = program(stage);
        if p.first().is_some_and(|w| base(w) == "xargs") {
            let skip = p[1..].iter().take_while(|a| a.starts_with('-')).count();
            p = program(&p[1 + skip..]);
        }
        let Some(w) = p.first() else { continue };
        let name = base(w);
        if n > 0 {
            out.push_str(" | ");
            out.push_str(name);
            continue;
        }
        out.push_str(name);
        if SUBCOMMANDS.contains(&name) {
            let mut it = p[1..].iter();
            while let Some(a) = it.next() {
                if !a.starts_with('-') {
                    out.push(' ');
                    out.push_str(a);
                    break;
                }
                if VALUE_FLAGS.contains(&a.as_str()) {
                    it.next();
                }
            }
        }
        for a in &p[1..] {
            if a.starts_with('-') {
                out.push(' ');
                out.push_str(a);
            }
        }
        if redirects(p) {
            out.push_str(" >");
        }
    }
    out
}

/// Drops env assignments and wrappers so the first word is the real program.
pub(super) fn program(stage: &[String]) -> &[String] {
    let mut s = stage;
    while let Some(w) = s.first() {
        let assign = w.contains('=') && !w.starts_with('-') && !w.starts_with('/');
        if assign || matches!(base(w), "sudo" | "time" | "env" | "command" | "nohup") {
            s = &s[1..];
        } else {
            break;
        }
    }
    s
}

pub(super) fn has_short_flag(args: &[String], c: char) -> bool {
    args.iter()
        .take_while(|a| a.starts_with('-'))
        .any(|a| !a.starts_with("--") && a.contains(c))
}

/// Whether a stage writes a file: a `>` or `>>` redirect to a path, or a `tee` operand.
pub(super) fn writes_file(p: &[String]) -> bool {
    if base(&p[0]) == "tee" {
        return p[1..]
            .iter()
            .any(|a| !a.starts_with('-') && a != "/dev/null");
    }
    redirects(p)
}

/// Whether a stage redirects its output to a real file (`>` or `>>`).
fn redirects(p: &[String]) -> bool {
    let mut words = p[1..].iter();
    while let Some(w) = words.next() {
        let Some(rest) = w
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .strip_prefix('>')
        else {
            continue;
        };
        let target = rest.trim_start_matches(['>', '|']);
        let target = if target.is_empty() {
            words.next().map_or("", String::as_str)
        } else {
            target
        };
        if !target.is_empty() && !target.starts_with('&') && target != "/dev/null" {
            return true;
        }
    }
    false
}
