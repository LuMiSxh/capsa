// SPDX-License-Identifier: MPL-2.0
//! Owns the interpreters and scripts that edit files in place.

use super::*;

pub(super) fn is_interpreter(head: &str) -> bool {
    head.starts_with("python") || matches!(head, "node" | "ruby" | "bun" | "deno")
}

/// Matches `cat > x.py <<EOF` or `tee x.py <<EOF`: the heredoc body is a script that runs in a
/// later call.
pub(super) fn script_writer(seg: &str) -> Option<&'static str> {
    let (head, words) = head(seg)?;
    if !matches!(head, "cat" | "tee") {
        return None;
    }
    words
        .map(|w| w.trim_matches(['\'', '"', '>']))
        .find_map(|w| match w.rsplit_once('.')?.1 {
            "py" => Some("python"),
            "js" | "mjs" | "cjs" => Some("node"),
            "rb" => Some("ruby"),
            _ => None,
        })
}

/// Matches a script that reads a source file, replaces text in it and writes it back: a hand-made
/// patch.
pub(super) fn is_edit_script(s: &str) -> bool {
    ["write(", "write_text(", "writeFileSync(", "writeFile("]
        .iter()
        .any(|w| s.contains(w))
        && [
            ".replace(",
            "re.sub(",
            ".index(",
            ".replaceAll(",
            ".gsub(",
            ".sub(",
        ]
        .iter()
        .any(|r| s.contains(r))
        && code_paths(s).next().is_some()
}

/// Yields the quoted literals of `s` that name a file with a code extension.
fn code_paths(s: &str) -> impl Iterator<Item = &str> {
    #[expect(
        clippy::unwrap_used,
        reason = "a constant pattern built from the code extensions"
    )]
    static CODE_PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(r#"['"][^'"\s]+\.({})['"]"#, CODE.join("|"))).unwrap()
    });
    CODE_PATH.find_iter(s).map(|m| m.as_str())
}

/// Returns the verdict for an edit script `s` run by `interp`, or `None` when `s` is not one. A
/// script whose every code-file literal is a source file inside the repository is refused; any
/// other one is only hinted at, because telling it from data processing is a heuristic.
pub(super) fn script_verdict(interp: &str, s: &str, scope: Option<&Scope>) -> Option<Verdict> {
    if !is_edit_script(s) {
        return None;
    }
    if code_paths(s).all(|p| is_repo_source(p, scope)) {
        return Some(Verdict::Deny(
            Rule::EditScript,
            format!(
                "Use acus instead of a {interp} script that edits source files: one `acus patch` with SEARCH/REPLACE blocks or `*** Replace All: PATHS` (literal or REGEX) edits every file at once, all or nothing, and prints the written lines. {ESCAPE}"
            ),
        ));
    }
    Some(Verdict::Hint(
        Rule::EditScript,
        format!(
            "acus hint: this {interp} script edits source files. Next time use one `acus patch` with SEARCH/REPLACE blocks or `*** Replace All: PATHS` (literal or REGEX): it edits every file at once, all or nothing, and prints the written lines."
        ),
    ))
}
