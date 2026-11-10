// SPDX-License-Identifier: MPL-2.0
//! Classifies shell commands and built-in tool calls into adoption categories.

use serde_json::Value;

/// The categories of one tool call, and the command that names each in `--misses`.
#[derive(Default)]
pub(crate) struct Hits {
    pub categories: Vec<&'static str>,
    pub commands: Vec<(&'static str, String)>,
}

impl Hits {
    fn named(category: &'static str, command: &str) -> Self {
        Hits {
            categories: vec![category],
            commands: vec![(category, command.to_owned())],
        }
    }

    fn shaped(pairs: Vec<(&'static str, Option<String>)>) -> Self {
        let mut hits = Hits::default();
        for (category, shape) in pairs {
            hits.categories.push(category);
            if let Some(shape) = shape {
                hits.commands.push((category, shape));
            }
        }
        hits
    }
}

/// Categories of one Claude tool call (`input` is the raw tool input JSON).
pub(crate) fn claude_tool(name: &str, input: &str) -> Hits {
    match name {
        "Read" => Hits::named("read", name),
        "Grep" | "Glob" => Hits::named("search", name),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Hits::named("edit-builtin", name),
        "Bash" => serde_json::from_str::<Value>(input)
            .ok()
            .and_then(|v| {
                let c = v.get("command")?.as_str()?;
                Some(Hits::shaped(classify_shaped(c)))
            })
            .unwrap_or_default(),
        _ => Hits::default(),
    }
}

/// Categories of one Codex tool call (`input` is its arguments or custom input).
pub(crate) fn codex_tool(name: &str, input: &str) -> Hits {
    if name == "apply_patch" {
        return Hits::named("edit-builtin", name);
    }
    let mut hits = Hits::default();
    for c in codex_commands(input) {
        for (category, shape) in classify_shaped(&c) {
            if hits.categories.contains(&category) {
                continue;
            }
            hits.categories.push(category);
            if let Some(shape) = shape {
                hits.commands.push((category, shape));
            }
        }
    }
    hits
}

/// The shell commands of one Codex tool call. Arguments are JSON, often a JSON string that holds
/// the JSON object again, so string literals are decoded until none is left. The `cmd` or
/// `command` value is a string or an argv array; other input (a JS tool body) is scanned for
/// `cmd` literals.
pub(crate) fn codex_commands(input: &str) -> Vec<String> {
    let mut args = input.to_owned();
    while let Ok(s) = serde_json::from_str::<String>(&args) {
        args = s;
    }
    let Ok(v) = serde_json::from_str::<Value>(&args) else {
        return js_cmds(&args);
    };
    match v.get("cmd").or_else(|| v.get("command")) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => vec![argv_command(a)],
        _ => js_cmds(&args),
    }
}

/// The script of an argv such as `["bash", "-lc", "cargo test"]`, or else the words joined.
fn argv_command(argv: &[Value]) -> String {
    let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
    let script = words
        .iter()
        .position(|w| matches!(*w, "-c" | "-lc"))
        .and_then(|i| words.get(i + 1));
    match script {
        Some(s) => (*s).to_owned(),
        None => words.join(" "),
    }
}

/// Extracts every `cmd` literal from a Codex JS shell call, such as
/// `tools.exec_command({cmd: "..."})`.
// ponytail: scans for `cmd: <string literal>`; computed commands are not seen.
fn js_cmds(js: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = js;
    while let Some(i) = rest.find("cmd") {
        rest = &rest[i + 3..];
        let t = rest.trim_start_matches(['"', '\'', ' ']);
        let Some(t) = t.strip_prefix(':') else {
            continue;
        };
        let mut cs = t.trim_start().chars();
        let Some(q @ ('"' | '\'' | '`')) = cs.next() else {
            continue;
        };
        let mut s = String::new();
        while let Some(c) = cs.next() {
            match c {
                c if c == q => break,
                '\\' => match cs.next() {
                    Some('n') => s.push('\n'),
                    Some(c) => s.push(c),
                    None => break,
                },
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

/// Splits a command into pipelines of stages at `&&`, `||`, `;`, newlines and `|`,
/// ignoring quoted text and heredoc bodies. The full text is returned for python code checks.
fn pipelines(cmd: &str) -> Vec<Vec<Vec<String>>> {
    let mut out = vec![vec![]];
    let mut words: Vec<String> = vec![];
    let (mut word, mut quote, mut has_word) = (String::new(), None::<char>, false);
    let mut heredoc: Option<String> = None;
    let cs: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    macro_rules! end_word {
        () => {{
            if has_word {
                words.push(std::mem::take(&mut word));
                has_word = false;
            }
        }};
    }
    macro_rules! end_stage {
        () => {{
            end_word!();
            if !words.is_empty() {
                out.last_mut().unwrap().push(std::mem::take(&mut words));
            }
        }};
    }
    macro_rules! end_pipeline {
        () => {{
            end_stage!();
            if !out.last().unwrap().is_empty() {
                out.push(vec![]);
            }
        }};
    }
    while i < cs.len() {
        let c = cs[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' && i + 1 < cs.len() {
                i += 1;
                word.push(cs[i]);
            } else {
                word.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                has_word = true;
            }
            '\\' if i + 1 < cs.len() => {
                i += 1;
                word.push(cs[i]);
                has_word = true;
            }
            '<' if cs.get(i + 1) == Some(&'<') && cs.get(i + 2) != Some(&'<') => {
                let (tag, next) = heredoc_tag(&cs, i);
                heredoc = Some(tag);
                i = next;
                continue;
            }
            '\n' => {
                end_pipeline!();
                if let Some(tag) = heredoc.take() {
                    i = skip_heredoc_body(&cs, i, &tag);
                }
            }
            ';' => end_pipeline!(),
            '&' if cs.get(i + 1) == Some(&'&') => {
                end_pipeline!();
                i += 1;
            }
            '|' if cs.get(i + 1) == Some(&'|') => {
                end_pipeline!();
                i += 1;
            }
            '|' => end_stage!(),
            c if c.is_whitespace() => end_word!(),
            c => {
                word.push(c);
                has_word = true;
            }
        }
        i += 1;
    }
    if has_word {
        words.push(word);
    }
    if !words.is_empty()
        && let Some(last) = out.last_mut()
    {
        last.push(words);
    }
    out.retain(|p| !p.is_empty());
    out
}

/// The delimiter of the heredoc that opens at `i`, and the index just past the opener.
fn heredoc_tag(cs: &[char], i: usize) -> (String, usize) {
    let mut j = i + 2;
    while cs
        .get(j)
        .is_some_and(|c| matches!(c, '-' | ' ' | '\'' | '"'))
    {
        j += 1;
    }
    let tag: String = cs[j..]
        .iter()
        .take_while(|c| c.is_alphanumeric() || **c == '_')
        .collect();
    let mut next = j + tag.chars().count();
    if matches!(cs.get(next), Some('\'' | '"')) {
        next += 1;
    }
    (tag, next)
}

/// The index where the heredoc body after line `i` ends: its line that is just `tag`, or the
/// end.
fn skip_heredoc_body(cs: &[char], i: usize, tag: &str) -> usize {
    let mut j = i + 1;
    while j <= cs.len() {
        let e = cs[j..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(cs.len(), |p| j + p);
        if cs[j..e].iter().collect::<String>().trim() == tag || e >= cs.len() {
            return e;
        }
        j = e + 1;
    }
    i
}

/// `edit-python` when the python code writes a file, `python-other` otherwise.
fn python_category(cmd: &str) -> &'static str {
    let writes = [".replace(", "re.sub", "write_text", ".write("]
        .iter()
        .any(|k| cmd.contains(k))
        || ["'w'", "\"w\""].iter().any(|k| cmd.contains(k)) && cmd.contains("open(");
    if writes {
        "edit-python"
    } else {
        "python-other"
    }
}

/// Whether a git command shows a diff: `git diff` or `git show`, after its `-C` and `-c`
/// options.
fn git_shows_diff(args: &[String]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-C" | "-c" => {
                it.next();
            }
            a if a.starts_with('-') => {}
            "diff" | "show" => return true,
            _ => break,
        }
    }
    false
}

fn base(w: &str) -> &str {
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
fn shape_pipe(pipe: &[Vec<String>]) -> String {
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
fn program(stage: &[String]) -> &[String] {
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

fn has_short_flag(args: &[String], c: char) -> bool {
    args.iter()
        .take_while(|a| a.starts_with('-'))
        .any(|a| !a.starts_with("--") && a.contains(c))
}

/// Whether a stage writes a file: a `>` or `>>` redirect to a path, or a `tee` operand.
fn writes_file(p: &[String]) -> bool {
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

const BUILD_TOOLS: [&str; 7] = [
    "cargo",
    "swift",
    "xcodebuild",
    "pytest",
    "make",
    "gradle",
    "gradlew",
];

fn is_build(p: &[String]) -> bool {
    let b = base(&p[0]);
    match b {
        "npm" | "pnpm" | "yarn" | "bun" => p
            .get(1)
            .is_some_and(|a| matches!(a.as_str(), "run" | "test" | "build")),
        "go" => p.get(1).is_some_and(|a| a == "test" || a == "build"),
        _ => BUILD_TOOLS.contains(&b),
    }
}

/// The activity a category belongs to; shares are computed within a group.
pub fn group(category: &str) -> &'static str {
    match category {
        "acus:find" | "acus:outline" | "acus:show" | "search" | "read" => "explore",
        "acus:patch" | "edit-sed" | "edit-python" | "edit-builtin" | "edit-shell" => "edit",
        "acus:ctx" | "build-filtered" | "build-unfiltered" => "build",
        "acus:diff" | "diff" => "diff",
        _ => "other",
    }
}

/// Every category a shell command hits, once each, with the shape of the pipeline that triggered
/// it.
pub(crate) fn classify_shaped(cmd: &str) -> Vec<(&'static str, Option<String>)> {
    let mut hits: Vec<(&'static str, Option<String>)> = vec![];
    for pipe in pipelines(cmd) {
        let shape = shape_pipe(&pipe);
        for (n, stage) in pipe.iter().enumerate() {
            let mut p = program(stage);
            // `find … | xargs grep` searches like a first stage.
            let mut first = n == 0;
            if p.first().is_some_and(|w| base(w) == "xargs") {
                let skip = p[1..].iter().take_while(|a| a.starts_with('-')).count();
                p = program(&p[1 + skip..]);
                first = true;
            }
            let Some(w) = p.first() else { continue };
            let args = &p[1..];
            let category = match base(w) {
                "acus" => Some(match args.first().map(String::as_str) {
                    Some("find") => "acus:find",
                    Some("outline") => "acus:outline",
                    Some("show") => "acus:show",
                    Some("patch") => "acus:patch",
                    Some("ctx") => "acus:ctx",
                    Some("diff") => "acus:diff",
                    Some("run") => "acus:run",
                    _ => "acus:other",
                }),
                "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack" | "find" | "fd" if first => {
                    Some("search")
                }
                "cat" | "printf" | "echo" | "tee" if writes_file(p) => Some("edit-shell"),
                "cat" | "head" | "tail" | "less" | "bat" | "nl" | "awk" if first => {
                    (base(w) != "awk" || args.iter().any(|a| a.contains("print"))).then_some("read")
                }
                "sed"
                    if has_short_flag(args, 'i')
                        || args.iter().any(|a| a.starts_with("--in-place")) =>
                {
                    Some("edit-sed")
                }
                "sed" if first && has_short_flag(args, 'n') => Some("read"),
                "perl" if has_short_flag(args, 'i') => Some("edit-sed"),
                "apply_patch" => Some("edit-builtin"),
                "git" => git_shows_diff(args).then_some("diff"),
                b if b == "python" || b.starts_with("python3") || b == "python2" => {
                    Some(python_category(cmd))
                }
                _ if is_build(p) => {
                    let filtered = pipe[n + 1..].iter().any(|s| {
                        matches!(
                            program(s).first().map(|w| base(w)),
                            Some("grep" | "egrep" | "rg" | "tail" | "head")
                        )
                    });
                    Some(if filtered {
                        "build-filtered"
                    } else {
                        "build-unfiltered"
                    })
                }
                _ => None,
            };
            if let Some(category) = category {
                add_hit(&mut hits, category, &shape);
            }
        }
    }
    hits
}

fn add_hit(hits: &mut Vec<(&'static str, Option<String>)>, category: &'static str, shape: &str) {
    if !hits.iter().any(|(c, _)| *c == category) {
        let command = if shape.is_empty() {
            None
        } else {
            Some(shape.to_owned())
        };
        hits.push((category, command));
    }
}

#[cfg(test)]
mod tests {
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
}
