// SPDX-License-Identifier: MPL-2.0
//! `Bash(...)` rules: exact commands, `*` wildcards and the `cmd:*` prefix form, checked against
//! the command and each of its subcommands.
//!
//! Approximations of Claude Code: the split is by quotes and the separators `&&`, `||`, `;`, `|`,
//! `&`, newline, parentheses, backticks and `$(`; `\*` is not an escape; the wrappers `time`,
//! `nohup`, `command`, `env`, `nice` and `timeout` are stripped, as are leading `NAME=value`, the
//! shell keywords `if`, `then`, `else`, `elif`, `do`, `while`, `until` and the `{` and `!` before a
//! command. Stricter than Claude Code, a tool cannot prompt: the quoted body of `bash -c`,
//! `sh -c` (also `zsh`, `dash`, `ksh`) and `eval` is checked as a command of its own. `xargs`,
//! `exec` and `find -exec` still run their argument unseen.

/// Returns whether `spec` covers `command` or any subcommand of it.
pub(super) fn matches(spec: &str, command: &str) -> bool {
    one(spec, command.trim())
        || segments(command)
            .iter()
            .any(|s| one(spec, s) || nested(s).iter().any(|body| matches(spec, body)))
}

/// Returns whether every subcommand of `command`, and of the bodies of its `bash -c` and `eval`
/// segments, is covered by one of the `specs`; a command with no subcommand is not.
pub(super) fn covered(specs: &[Option<&str>], command: &str) -> bool {
    let parts = segments(command);
    !parts.is_empty()
        && parts.iter().all(|s| {
            specs.iter().flatten().any(|spec| one(spec, s))
                && nested(s).iter().all(|body| covered(specs, body))
        })
}

/// Returns the script texts a `bash -c '...'` or `eval ...` segment runs, without their quotes.
fn nested(segment: &str) -> Vec<String> {
    let (word, rest) = split_word(segment);
    let name = word.rsplit('/').next().unwrap_or(word);
    match name {
        "eval" => vec![unquote(rest)],
        "bash" | "sh" | "zsh" | "dash" | "ksh" => shell_body(rest),
        _ => Vec::new(),
    }
}

/// Long options of the shells that take no argument; any other `--option` leaves the argument
/// structure unknown.
const LONG_FLAGS: [&str; 14] = [
    "login",
    "norc",
    "noprofile",
    "posix",
    "restricted",
    "verbose",
    "noediting",
    "debugger",
    "version",
    "help",
    "pretty-print",
    "dump-strings",
    "dump-po-strings",
    "rpm-requires",
];

/// Returns the `-c` body in the arguments of a shell: the first word that is no option, once a
/// flag with `c` (`-c`, `-ec`, `-lc`) has been seen. `-o`, `-O`, `+o`, `+O`, `--rcfile` and
/// `--init-file` take an argument, and `--` ends the options. When the options cannot be told
/// apart, every word after the first `-c` counts as a body, so that a rule is not skipped.
fn shell_body(args: &str) -> Vec<String> {
    let (mut rest, mut command) = (args, false);
    loop {
        let (flag, after) = split_word(rest);
        if flag == "--" || !flag.starts_with(['-', '+']) {
            let body = if flag == "--" { after } else { rest };
            return if command && !body.is_empty() {
                vec![unquote(body)]
            } else {
                Vec::new()
            };
        }
        rest = after;
        if let Some(long) = flag.strip_prefix("--") {
            match long {
                "rcfile" | "init-file" => rest = split_word(rest).1,
                l if LONG_FLAGS.contains(&l) => {}
                _ => return every_word_after_c(args),
            }
            continue;
        }
        for letter in flag.chars().skip(1) {
            match letter {
                'c' if flag.starts_with('-') => command = true,
                'o' | 'O' => rest = split_word(rest).1,
                _ => {}
            }
        }
    }
}

/// Returns each word after the first single-dash flag with a `c` as a body.
fn every_word_after_c(args: &str) -> Vec<String> {
    let words = words(args);
    let first = words
        .iter()
        .position(|w| w.starts_with('-') && !w.starts_with("--") && w.contains('c'));
    first.map_or_else(Vec::new, |i| words[i + 1..].to_vec())
}

/// Splits `text` into words at whitespace outside quotes, with the quotes removed.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Returns the first word of `text` without its quotes; unquoted text is returned as it is.
fn unquote(text: &str) -> String {
    let text = text.trim();
    let Some(quote) = text.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
        return text.to_owned();
    };
    let mut body = String::new();
    let mut chars = text[1..].chars();
    while let Some(c) = chars.next() {
        if c == quote {
            break;
        }
        if c == '\x5c' && quote == '"' {
            if let Some(n) = chars.next() {
                body.push(n);
            }
            continue;
        }
        body.push(c);
    }
    body
}

/// Matches one command against a spec. `cmd:*` and `cmd *` also match `cmd` with no arguments,
/// where a bare `cmd` matches only itself.
fn one(spec: &str, command: &str) -> bool {
    if let Some(prefix) = spec.strip_suffix(":*") {
        return glob(prefix, command) || glob(&format!("{prefix} *"), command);
    }
    if let Some(prefix) = spec.strip_suffix(" *")
        && glob(prefix, command)
    {
        return true;
    }
    glob(spec, command)
}

/// Matches `text` against `pattern`, where `*` stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if p.get(pi) == Some(&'*') {
            star = Some((pi, ti));
            pi += 1;
        } else if p.get(pi).is_some_and(|c| *c == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi.min(p.len())..].iter().all(|c| *c == '*')
}

/// Returns the subcommands of `command`, each with its leading assignments and wrappers removed.
pub(super) fn segments(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut single, mut double) = (false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1).copied();
        if single {
            single = c != '\'';
            cur.push(c);
        } else {
            match c {
                '\\' => {
                    cur.push(c);
                    if let Some(n) = next {
                        cur.push(n);
                        i += 1;
                    }
                }
                '\'' if !double => {
                    single = true;
                    cur.push(c);
                }
                '"' => {
                    double = !double;
                    cur.push(c);
                }
                '`' => cut(&mut out, &mut cur),
                '$' if next == Some('(') => {
                    cut(&mut out, &mut cur);
                    i += 1;
                }
                '&' if prev == Some('>') || next == Some('>') => cur.push(c),
                ';' | '|' | '&' | '\n' | '(' | ')' if !double => cut(&mut out, &mut cur),
                _ => cur.push(c),
            }
        }
        i += 1;
    }
    cut(&mut out, &mut cur);
    out
}

fn cut(out: &mut Vec<String>, cur: &mut String) {
    let text = bare(cur.trim());
    if !text.is_empty() {
        out.push(text.to_owned());
    }
    cur.clear();
}

/// Returns the command without the assignments and wrapper commands before it.
fn bare(segment: &str) -> &str {
    let mut rest = segment;
    loop {
        let (word, after) = split_word(rest);
        rest = match word {
            "time" | "nohup" | "command" | "env" | "if" | "then" | "else" | "elif" | "do"
            | "while" | "until" | "{" | "!" => after,
            "nice" => nice(after),
            "timeout" => timeout(after),
            w if is_assignment(w) => after,
            _ => return rest,
        };
    }
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn nice(after: &str) -> &str {
    let (flag, rest) = split_word(after);
    if flag == "-n" {
        split_word(rest).1
    } else if flag.len() > 1
        && flag.starts_with('-')
        && flag[1..].chars().all(|c| c.is_ascii_digit())
    {
        rest
    } else {
        after
    }
}

fn timeout(after: &str) -> &str {
    let mut rest = after;
    loop {
        let (flag, next) = split_word(rest);
        if !flag.starts_with('-') {
            break;
        }
        rest = if matches!(flag, "-s" | "-k" | "--signal" | "--kill-after") {
            split_word(next).1
        } else {
            next
        };
    }
    let (duration, next) = split_word(rest);
    if duration.starts_with(|c: char| c.is_ascii_digit()) {
        next
    } else {
        rest
    }
}
