// SPDX-License-Identifier: MPL-2.0

//! Owns the shell searches (grep, rg, find, the search tool) and their acus find equivalents.

use super::*;

mod lex;
mod parse;

pub(super) use lex::*;
pub(super) use parse::*;

/// Holds a search command's pattern, targets and the `acus find` flags that mean the same.
#[derive(Default)]
pub(super) struct Search<'a> {
    pub(super) pattern: Option<String>,
    pub(super) operands: Vec<&'a str>,
    /// Searches the working directory when it names no file.
    pub(super) recursive: bool,
    pub(super) flags: Vec<String>,
    /// A flag, pattern source or output mode that `acus find` cannot reproduce exactly.
    pub(super) inexact: bool,
}

/// Holds one word of a grep or rg command line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GrepOpt<'a> {
    /// A long option and its value.
    Long(&'a str, Option<&'a str>),
    /// One letter of a short cluster and, for a letter that takes one, its value.
    Short(char, Option<&'a str>),
    /// `-NUM`, which is `-C NUM`.
    Context(&'a str),
    /// A lone `-`: standard input.
    Stdin,
    /// A plain word, or anything after `--`.
    Word(&'a str),
}

/// Holds the parse state that the per-tool readers share.
struct Parsed<'a> {
    s: Search<'a>,
    have_pattern: bool,
    ere: bool,
    /// `grep`, `egrep` or `fgrep`.
    grep: bool,
    /// Exactly `rg`: `ag` and `ack` give `-L`, `-U` and `-x` other meanings.
    rg: bool,
    /// A `--` was read: the words after it are no options.
    after_dashes: bool,
}

impl Parsed<'_> {
    /// Matches a flag with the same meaning for the tools in `ok`.
    fn flag_if(&mut self, flag: &str, ok: bool) {
        if ok {
            self.s.flags.push(flag.into());
        } else {
            self.s.inexact = true;
        }
    }

    /// A second pattern cannot be reproduced.
    fn set_pattern(&mut self, p: Option<String>) {
        self.s.inexact |= self.have_pattern;
        self.have_pattern = true;
        self.s.pattern = p;
    }

    /// Patterns read from a file are not something `acus find` takes.
    fn patterns_from_file(&mut self) {
        self.s.inexact = true;
        self.have_pattern = true;
    }
}

/// Matches PowerShell's way to grep: `Get-ChildItem … | Select-String PAT` searches the contents
/// of the files listed. Narrowed to plain files (`-Include *.log`), it is data and passes.
pub(super) fn listed_search(seg: &Segment, next: &Segment, ps: bool) -> Option<Verdict> {
    let (filter, fargs) = head(&next.orig).filter(|_| ps && seg.feeds(next))?;
    let (lister, largs) = head(seg.orig_unescaped().1)?;
    let (filter, lister) = (filter.to_lowercase(), lister.to_lowercase());
    if !matches!(filter.as_str(), "select-string" | "sls")
        || !matches!(lister.as_str(), "get-childitem" | "gci" | "ls" | "dir")
    {
        return None;
    }
    let (mut paths, mut globs) = (Vec::new(), Vec::new());
    let mut words = largs;
    while let Some(a) = words.next() {
        match a.to_lowercase().as_str() {
            // Only names are listed, nothing is searched.
            "-name" => return None,
            "-filter" | "-include" => {
                globs.extend(
                    words
                        .next()
                        .into_iter()
                        .flat_map(|g| g.split(','))
                        .map(unquote),
                );
            }
            "-path" | "-literalpath" => paths.extend(words.next().map(unquote)),
            "-exclude" | "-depth" | "-attributes" => {
                words.next();
            }
            l if l.starts_with(['-', '/']) => {}
            _ => paths.push(unquote(a)),
        }
    }
    let named = if globs.is_empty() { &paths } else { &globs };
    if !named.is_empty() && named.iter().all(|n| is_plain_file(n)) {
        return None;
    }
    let fargs: Vec<&str> = fargs.collect();
    let mut search = parse_search(&filter, &fargs);
    search.operands = paths
        .into_iter()
        .filter(|p| !matches!(*p, "." | ".\\"))
        .collect();
    search
        .flags
        .extend(globs.iter().map(|g| format!("-g {}", quote(g))));
    Some(Verdict::Deny(
        Rule::Grep,
        format!(
            "Use acus instead of piping {lister} into {filter}: `{}` searches the same files and groups the hits by file and enclosing symbol. {ESCAPE}",
            find_cmd(&search)
        ),
    ))
}

/// Lists the commands that search file contents, PowerShell and cmd.exe names included.
pub(super) const GREPS: &[&str] = &[
    "grep",
    "rg",
    "egrep",
    "fgrep",
    "ag",
    "ack",
    "select-string",
    "sls",
    "findstr",
];

pub(super) fn grep_reason(head: &str) -> String {
    format!(
        "Use acus instead of {head}: `acus find 'PAT' [PATH…]` (`--block` adds the enclosing code, `-l` lists files, `-w`, `-t TYPE`, `-g GLOB`, `-u --hidden` include ignored and hidden files). {ESCAPE}"
    )
}

/// A count prints one number per file, not the lines: `grep -c PAT FILE` runs as written.
pub(super) fn is_count(s: &Search) -> bool {
    s.flags.iter().any(|f| f == "-c")
}

/// Reports whether a search is aimed at source: it names a directory, a glob or a source file, or
/// walks the working directory. A search of stdin or of plain files is left alone.
pub(super) fn aims_at_source(s: &Search, via_xargs: bool) -> bool {
    via_xargs
        || if s.operands.is_empty() {
            s.recursive
        } else {
            !s.operands.iter().all(|o| is_plain_file(o))
        }
}

/// Returns the `acus find` call that does what `s` did.
pub(super) fn find_cmd(s: &Search) -> String {
    let pat = s.pattern.as_deref().unwrap_or("PAT");
    // A pattern that starts with `-` would read as a flag.
    let mut cmd = if pat.starts_with('-') {
        format!("acus find -e {}", quote(pat))
    } else {
        format!("acus find {}", quote(pat))
    };
    for o in &s.operands {
        cmd += &format!(" {}", path_addr(o, ""));
    }
    for f in &s.flags {
        cmd += &format!(" {f}");
    }
    cmd
}

/// Refuses the built-in Grep tool with the equivalent `acus find`. Counts and multiline patterns,
/// which acus does not do, pass.
pub fn grep_tool(input: &Value, cwd: Option<&str>) -> Option<Verdict> {
    let mode = input["output_mode"]
        .as_str()
        .unwrap_or("files_with_matches");
    if mode == "count" || input["multiline"].as_bool() == Some(true) {
        return None;
    }
    let mut cmd = format!("acus find {}", quote(input["pattern"].as_str()?));
    if let Some(p) = input["path"].as_str().map(|p| relative(p, cwd))
        && p != "."
    {
        cmd += &format!(" {}", quote(&p));
    }
    if let Some(g) = input["glob"].as_str() {
        cmd += &format!(" -g {}", quote(g));
    }
    if let Some(t) = input["type"].as_str() {
        cmd += &format!(" -t {}", quote(t));
    }
    if input["-i"].as_bool() == Some(true) {
        cmd += " -i";
    }
    if mode == "files_with_matches" {
        cmd += " -l";
    } else if ["-A", "-B", "-C", "context"]
        .iter()
        .any(|k| !input[k].is_null())
    {
        cmd += " --block";
    }
    Some(Verdict::Deny(
        Rule::GrepTool,
        format!(
            "Use acus instead of the Grep tool: `{cmd}` groups the hits by file and enclosing symbol, and names the follow-up command for anything it cuts. Only if acus cannot do this, run `command rg` via Bash."
        ),
    ))
}

/// Reports whether output is sent to a file (`> x`, `>> x`, `1> x`), which makes a command a copy
/// or an append rather than a read. Stderr redirects do not count.
pub(super) fn redirects_stdout(args: &[&str]) -> bool {
    // `>`, `>>`, `>|`, `>&2` and `&>`, with an optional `1`; `2>` and `2>&1` leave stdout alone.
    args.iter().any(|a| {
        let a = a.trim_start_matches('1');
        a.starts_with('>') || a.starts_with("&>")
    })
}

/// Reports a grep, rg or findstr call with `-q`: only its exit status is used.
pub(super) fn is_quiet(args: &[&str]) -> bool {
    args.iter().any(|a| {
        matches!(*a, "--quiet" | "--silent" | "-quiet")
            || (a.starts_with('-') && !a.starts_with("--") && {
                a[1..]
                    .chars()
                    .take_while(|c| !"efmABCgt".contains(*c))
                    .any(|c| c == 'q')
            })
    })
}

#[cfg(all(test, any(feature = "bash", feature = "zsh")))]
mod brace_tests {
    use crate::rewrite::rewrite;

    // tests::rw is private to the tests module, so this mirrors it.
    fn rw(cmd: &str) -> Option<String> {
        rewrite(cmd, None).map(|r| r.command)
    }

    fn assert_rewrites(cases: &[(&str, &str)]) {
        for (cmd, want) in cases {
            assert_eq!(rw(cmd).as_deref(), Some(*want), "{cmd}");
        }
    }

    #[test]
    fn plain_brace_lists_expand_to_one_glob_each() {
        assert_rewrites(&[
            (
                "grep -rn foo --exclude-dir=target .",
                "acus find 'foo' '.' -g '!target'",
            ),
            (
                "grep -rn foo --exclude-dir={target,node_modules} .",
                "acus find 'foo' '.' -g '!target' -g '!node_modules'",
            ),
            (
                "grep -rn foo --include={a.rs,b.md} --exclude={x.log,y-1} .",
                "acus find 'foo' '.' -g 'a.rs' -g 'b.md' -g '!x.log' -g '!y-1'",
            ),
        ]);
    }

    // The shell strips the quotes of a fully quoted option word, so grep reads it as the unquoted
    // option would be read.
    #[test]
    fn quoted_option_word_is_an_option() {
        for (quoted, plain) in [
            ("grep -rn foo '-i' .", "grep -rn foo -i ."),
            (
                "grep -rn foo \"--exclude-dir=target\" .",
                "grep -rn foo --exclude-dir=target .",
            ),
            ("grep -r \"-n\" x .", "grep -r -n x ."),
        ] {
            assert!(rw(plain).is_some(), "{plain}");
            assert_eq!(rw(quoted), rw(plain), "{quoted}");
        }
    }

    // Braces stay literal in a quoted word, so grep's meaning is not a glob list: refused.
    #[test]
    fn quoted_option_word_with_brace_is_refused() {
        for cmd in [
            "grep -rn foo '--exclude-dir={target,node_modules}' .",
            "grep -rn foo \"--exclude-dir={a,b}\" .",
        ] {
            assert_eq!(rw(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn other_brace_forms_stay_refused() {
        for cmd in [
            "grep -rn foo --exclude-dir='{target,node_modules}' .",
            "grep -rn foo --exclude-dir={a/b,c} .",
            "grep -rn foo --exclude-dir={a,,c} .",
            "grep -rn foo --exclude-dir={a,{b,c}} .",
            "grep -rn foo --exclude-dir={a} .",
            "grep -rn foo --include={*.rs,b} .",
        ] {
            assert_eq!(rw(cmd), None, "{cmd}");
        }
    }
}

#[cfg(test)]
mod lex_tests {
    use super::*;
    use GrepOpt::*;

    #[test]
    fn lexes_clusters_values_and_context() {
        assert_eq!(
            lex_grep(&["-rnA2", "-e", "x", "--include=*.rs", "-5", "--", "-v"]),
            vec![
                Short('r', None),
                Short('n', None),
                Short('A', Some("2")),
                Short('e', Some("x")),
                Long("include", Some("*.rs")),
                Context("5"),
                Word("-v")
            ]
        );
        assert_eq!(
            lex_grep(&["--max-count", "3", "-", "pat"]),
            vec![Long("max-count", Some("3")), Stdin, Word("pat")]
        );
    }
}
