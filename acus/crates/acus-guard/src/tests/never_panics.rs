// SPDX-License-Identifier: MPL-2.0
//! Guard tests that odd input never panics.

use super::*;

#[test]
fn never_panics_on_odd_input() {
    for cmd in [
        "",
        " ",
        "\"",
        "'",
        "`",
        "\\",
        "&",
        "|",
        "||",
        "&&",
        ";",
        "(",
        ")",
        "<",
        ">",
        "<<<",
        "<<",
        "<<EOF",
        "grep",
        "grep -e",
        "grep --regexp",
        "grep --regexp=",
        "grep -",
        "grep --",
        "grep -A",
        "grep -efoo",
        "rg -g",
        "findstr",
        "findstr /",
        "findstr /c",
        "findstr /c:",
        "findstr /é",
        "findstr /éa:x",
        "sls -pattern",
        "sls -path",
        "sls -",
        "gci -filter",
        "gci | sls",
        "head -n",
        "head -é",
        "sed -n",
        "sed -n p",
        "cat >",
        "cat 2>",
        "xargs",
        "find -exec",
        "env",
        "uv run",
        "cd",
        "é",
        "日本語 grep 日本語.rs",
        "gc ,",
        "gc ,a.rs",
        "ls -",
        "ls -r | sls -",
        "'a b",
        "\"a b",
        "cat 'a.rs",
        "cat \"a.rs",
    ] {
        let _ = check(cmd);
        let _ = check_ps(cmd, None);
        let _ = check_ps(&format!("cd x; {cmd}; {cmd} | {cmd}"), None);
    }
}
