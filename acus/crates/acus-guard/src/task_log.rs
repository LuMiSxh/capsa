// SPDX-License-Identifier: MPL-2.0
//! Folds a read of a background task's output file into one `acus ctx --log` call.

use super::*;

/// Reports whether `path` is `/…/tasks/NAME.output` with NAME made of `[A-Za-z0-9_-]`. Checked on
/// the text, not the file system, so it holds on every platform.
fn is_task_output(path: &str) -> bool {
    // Every byte is a plain path byte: a glob, `~` or `$` would no longer expand once quoted.
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-'))
    {
        return false;
    }
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    let mut parts = rest.rsplit('/');
    let (Some(file), Some(dir)) = (parts.next(), parts.next()) else {
        return false;
    };
    dir == "tasks"
        && file.strip_suffix(".output").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
}

/// Folds a task-log segment at `i` and its following filters. Returns (last folded index,
/// replacement text for `segs[i..=last]`).
pub(crate) fn fold(segs: &[Segment], i: usize) -> Option<(usize, String)> {
    let seg = &segs[i];
    // PowerShell lowercases `low`, which would change the path.
    if seg.low != seg.orig || seg.stdin || seg.opaque() {
        return None;
    }
    let (escaped, plain) = seg.low_unescaped();
    if escaped {
        return None;
    }
    let words = split_words(seg.command_part(plain));
    let (&cmd, rest) = words.split_first()?;
    let (&file, args) = rest.split_last()?;
    let file = unquote(file);
    if !is_task_output(file) {
        return None;
    }
    let log = format!("acus ctx --log {}", quote(file));
    let last = filter_last(segs, i);
    let end = &segs[last];
    if (last != i && cmd != "cat") || end.out != Out::Agent || end.gates || continues(segs, last) {
        return None;
    }
    let new = match cmd {
        "cat" if args.is_empty() => {
            let filters: Vec<&str> = segs[i + 1..=last].iter().map(|s| s.low.as_str()).collect();
            if filters.is_empty() {
                log
            } else {
                rewrite_ctx(&log, &filters)?
            }
        }
        "tail" => match window("tail", args)? {
            Window::Last(n) => format!("{log} --tail {n}"),
            _ => return None,
        },
        "grep" => {
            let (flag, _) = ctx_grep_flag(args)?;
            format!("{log} {flag}")
        }
        _ => return None,
    };
    Some((last, new))
}

// The task output paths are Unix paths, so the tests run on Unix only.
#[cfg(all(test, unix, any(feature = "bash", feature = "zsh")))]
mod tests {
    fn rw(cmd: &str) -> Option<String> {
        crate::rewrite::rewrite(cmd, None).map(|r| r.command)
    }

    const F: &str = "/private/tmp/agent/proj/tasks/b7k2p9.output";

    #[test]
    #[cfg(unix)]
    fn reads_of_a_task_output_become_ctx_log() {
        assert_eq!(
            rw(&format!("cat {F}")),
            Some(format!("acus ctx --log '{F}'"))
        );
        assert_eq!(
            rw(&format!("tail -n 40 {F}")),
            Some(format!("acus ctx --log '{F}' --tail 40"))
        );
        assert_eq!(
            rw(&format!("cat '{F}'")),
            Some(format!("acus ctx --log '{F}'"))
        );
        let g = rw(&format!("grep -n error {F}")).unwrap();
        assert!(
            g.starts_with(&format!("acus ctx --log '{F}' --grep")),
            "{g}"
        );
        let p = rw(&format!("cat {F} | grep -v warn | tail -5")).unwrap();
        assert!(p.starts_with(&format!("acus ctx --log '{F}' ")), "{p}");
        assert!(p.contains("--drop") && p.contains("--tail 5"), "{p}");
    }

    #[test]
    #[cfg(unix)]
    fn other_shapes_stay_as_they_are() {
        let tasks = |cmd: &str| rw(cmd).is_some_and(|c| c.contains("--log"));
        for cmd in [
            format!("tail -f {F}"),
            format!("tail -n +5 {F}"),
            "cat /private/tmp/agent/proj/notes/b7k2p9.output".to_owned(),
            "cat /private/tmp/agent/proj/tasks/b7k2p9.txt".to_owned(),
            "cat /private/tmp/agent/proj/tasks/a.b.output".to_owned(),
            "cat /private/tmp/agent/*/tasks/b7k2p9.output | tail -30".to_owned(),
            "tail -5 /private/tmp/*/proj/tasks/b7k2p9.output".to_owned(),
            "cat tasks/b7k2p9.output".to_owned(),
            format!("cat {F} > copy.txt"),
            format!("cat -n {F}"),
            format!("grep -c error {F}"),
            format!("grep -l error {F}"),
            format!("grep -o error {F}"),
            format!("grep -q error {F}"),
            format!("cat {F} | wc -l"),
            format!("cat {F} && echo done"),
            format!("x=$(cat {F})"),
        ] {
            assert!(!tasks(&cmd), "{cmd}");
        }
    }
}
