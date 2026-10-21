// SPDX-License-Identifier: MPL-2.0
//! Folds a whole-command `sed -i` into an `acus patch` heredoc.
//!
//! Only the literal form `s/OLD/NEW/g` on one plain file converts. Every check errs towards
//! refusing, because a wrong conversion edits the user's files.

use super::*;

/// Holds a word of the command: its text without quotes, and whether it was single-quoted.
struct Tok<'a> {
    text: &'a str,
    quoted: bool,
}

fn bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./,:=@%+-".contains(c)
}

/// Splits `s` into words made of safe bare characters, `'..'` without a backslash, or `""`.
/// Anything else (a `$`, a redirect, a backslash, a quote glued to text) refuses the line.
fn words(s: &str) -> Option<Vec<Tok<'_>>> {
    let mut out = Vec::new();
    let mut rest = s.trim_matches([' ', '\t']);
    while !rest.is_empty() {
        let (tok, len) = if let Some(body) = rest.strip_prefix('\'') {
            let end = body.find('\'')?;
            (
                Tok {
                    text: &body[..end],
                    quoted: true,
                },
                end + 2,
            )
        } else if rest.starts_with("\"\"") {
            (
                Tok {
                    text: "",
                    quoted: true,
                },
                2,
            )
        } else {
            let end = rest.find(|c: char| !bare(c)).unwrap_or(rest.len());
            if end == 0 {
                // A quote, `$`, `>`, newline or backslash: not a word this fold knows.
                if !rest.starts_with([' ', '\t']) {
                    return None;
                }
                rest = rest.trim_start_matches([' ', '\t']);
                continue;
            }
            (
                Tok {
                    text: &rest[..end],
                    quoted: false,
                },
                end,
            )
        };
        rest = &rest[len..];
        // A word ends at a space; `'a'b` would be one word to the shell.
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            return None;
        }
        out.push(tok);
    }
    Some(out)
}

/// Reports whether `s` can sit on one line of a patch block as search or replace text.
fn plain(s: &str) -> bool {
    !s.is_empty()
        && s.trim() == s
        && !s.chars().any(char::is_control)
        && !["<<<<<<<", "=======", ">>>>>>>", "*** "]
            .iter()
            .any(|p| s.starts_with(p))
        && s != "ACUS_SED_END"
}

/// Parses `s D OLD D NEW D g` with literal OLD and NEW, as (OLD, NEW).
fn literal_subst(script: &str) -> Option<(&str, &str)> {
    let body = script.strip_prefix('s')?;
    let d = body.chars().next()?;
    if !d.is_ascii() || d.is_ascii_alphanumeric() || d.is_control() || d == '\\' || d == ' ' {
        return None;
    }
    let mut parts = body[1..].split(d);
    let (old, new, flags) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || flags != "g" {
        return None;
    }
    let literal_old = !old.contains([
        '.', '[', ']', '*', '^', '$', '+', '?', '(', ')', '{', '}', '|', '\\',
    ]);
    let literal_new = !new.contains(['&', '\\']);
    (literal_old && literal_new && plain(old) && plain(new)).then_some((old, new))
}

/// Reports whether sed and the patch act alike on `file`: a regular file (both edit it) or a
/// missing path (both fail and write nothing). Directories and symlinks differ: sed refuses them,
/// while Replace All walks a directory or writes through a link. The path is read against the Bash
/// call's cwd; a scope that cannot read the disk gives no answer, so the line is refused.
fn fits_sed(file: &str, scope: Option<&Scope>) -> bool {
    let path = match scope {
        Some(s) => match s.path(file) {
            Some(p) => p,
            None => return false,
        },
        None => std::path::PathBuf::from(file),
    };
    match std::fs::symlink_metadata(&path) {
        Ok(m) => m.file_type().is_file(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Folds a whole-command `sed -i` into an `acus patch` heredoc; only when `segs.len() == 1`.
pub(crate) fn fold(segs: &[Segment], i: usize, scope: Option<&Scope>) -> Option<String> {
    let [seg] = segs else { return None };
    if i != 0
        || seg.stdin
        || seg.gates
        || seg.heredoc
        || seg.assigns != 0
        || !matches!(seg.out, Out::Agent)
        || !matches!(seg.nest, Nest::Top)
    {
        return None;
    }
    let toks = words(&seg.orig)?;
    let mut it = toks.iter().peekable();
    if it.next().is_none_or(|t| t.quoted || t.text != "sed")
        || it.next().is_none_or(|t| t.quoted || t.text != "-i")
    {
        return None;
    }
    // BSD sed's empty backup suffix.
    let _ = it.next_if(|t| t.quoted && t.text.is_empty());
    let _ = it.next_if(|t| !t.quoted && t.text == "-e");
    let script = it.next()?;
    let file = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let (old, new) = literal_subst(script.text)?;
    let f = file.text;
    if file.quoted
        || f.is_empty()
        || f.starts_with('-')
        || !f
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
        || !fits_sed(f, scope)
    {
        return None;
    }
    Some(format!(
        "acus patch <<'ACUS_SED_END'\n*** Begin Patch\n*** Replace All: {f}\n<<<<<<< SEARCH\n{old}\n=======\n{new}\n>>>>>>> REPLACE\n*** End Patch\nACUS_SED_END"
    ))
}

#[cfg(all(test, any(feature = "bash", feature = "zsh")))]
mod tests {
    use crate::Scope;
    use crate::rewrite::rewrite;

    fn rw(cmd: &str) -> Option<String> {
        rewrite(cmd, None).map(|r| r.command)
    }

    fn patch(old: &str, new: &str, file: &str) -> String {
        format!(
            "acus patch <<'ACUS_SED_END'\n*** Begin Patch\n*** Replace All: {file}\n<<<<<<< SEARCH\n{old}\n=======\n{new}\n>>>>>>> REPLACE\n*** End Patch\nACUS_SED_END"
        )
    }

    #[test]
    fn literal_forms_become_patches() {
        let want = patch("old_name", "new_name", "src/a.rs");
        for cmd in [
            "sed -i '' 's/old_name/new_name/g' src/a.rs",
            "sed -i \"\" 's/old_name/new_name/g' src/a.rs",
            "sed -i 's|old_name|new_name|g' src/a.rs",
            "sed -i -e 's/old_name/new_name/g' src/a.rs",
            "sed -i '' -e 's/old_name/new_name/g' src/a.rs",
            "sed -i s/old_name/new_name/g src/a.rs",
            "sed -i 's,old_name,new_name,g' src/a.rs",
        ] {
            assert_eq!(rw(cmd).as_deref(), Some(want.as_str()), "{cmd}");
        }
        assert_eq!(
            rw("sed -i '' 's/a b/c d/g' x.txt").as_deref(),
            Some(patch("a b", "c d", "x.txt").as_str())
        );
        // Single quotes keep these literal in sed, and the heredoc delimiter is quoted too.
        assert_eq!(
            rw("sed -i '' 's/a/$HOME`id`/g' x.txt").as_deref(),
            Some(patch("a", "$HOME`id`", "x.txt").as_str())
        );
        let r = rewrite("sed -i '' 's/old_name/new_name/g' src/a.rs", None).unwrap();
        assert!(r.deferred);
        assert!(r.note.contains("writes nothing when the text is not found"));
    }

    #[expect(clippy::unwrap_used, reason = "test helper")]
    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("acus-sed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("dir/sub")).unwrap();
        std::fs::write(d.join("real.txt"), "a\n").unwrap();
        d
    }

    #[test]
    fn only_regular_or_missing_files_convert() {
        let d = tmp("kinds");
        let scope = Scope::new(d.to_str());
        let at = |f: &str| {
            rewrite(&format!("sed -i '' 's/a/b/g' {f}"), scope.as_ref())
                .is_some_and(|r| r.command.starts_with("acus patch"))
        };
        assert!(at("real.txt"));
        assert!(at("missing.txt"));
        assert!(!at("dir"));
        assert!(!at("dir/sub"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("real.txt", d.join("link.txt")).unwrap();
            std::os::unix::fs::symlink("dir", d.join("dirlink")).unwrap();
            std::os::unix::fs::symlink("nowhere", d.join("dangling")).unwrap();
            assert!(!at("link.txt"));
            assert!(!at("dirlink"));
            assert!(!at("dangling"));
        }

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn relative_names_follow_the_scope_cwd() {
        let d = tmp("cwd");
        // `src` is a directory in the process cwd only; in the scope it does not exist.
        let scope = Scope::new(d.to_str());
        let r = rewrite("sed -i '' 's/a/b/g' src", scope.as_ref());
        assert!(r.is_some_and(|r| r.command.starts_with("acus patch")));
        // `dir` is a directory in the scope only.
        assert!(rw("sed -i '' 's/a/b/g' dir").is_some_and(|c| c.starts_with("acus patch")));
        let r = rewrite("sed -i '' 's/a/b/g' dir", scope.as_ref());
        assert!(r.is_none_or(|r| !r.command.starts_with("acus patch")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn everything_else_keeps_its_verdict() {
        for bad in [
            "sed -i '' 's/a.b/c/g' src/a.rs",
            "sed -i '' 's/a/b/' src/a.rs",
            "sed -i '' 's/a/b/gp' src/a.rs",
            "sed -i '' 's/a/&x/g' src/a.rs",
            "sed -i '' \"s/a/b/g\" src/a.rs",
            "sed -i.bak 's/a/b/g' src/a.rs",
            "sed -i '' 's/a/b/g' src/a.rs src/b.rs",
            "sed -i '' 's/a/b/g' src/a.rs && cargo build",
            "sed -i '' 's/a/b/g' src/a.rs; ls",
            "sed -i '' 's/a/b/g' src/a.rs | cat",
            "sed -i '' 's/a/b/g' src/a.rs > out",
            "sed -i '' 's/a/b/g' src/a.rs &",
            "sed -i '' 's/a/b/g' src/a.rs # note",
            "sed -i '' 's/a/b/g' $F",
            "sed -i '' 's/a/b/g' 'src/a.rs'",
            "sed -i '' 's/a\\/b/c/g' src/a.rs",
            "sed -i '' 's/a/b\\n/g' src/a.rs",
            "sed -i '' 's/a/ b/g' src/a.rs",
            "sed -i '' 's//b/g' src/a.rs",
            "sed -i '' 's/a//g' src/a.rs",
            "sed -i '' 's/<<<<<<< x/b/g' src/a.rs",
            "sed -i '' 's/a/*** End Patch/g' src/a.rs",
            "sed -i '' 's/ACUS_SED_END/b/g' src/a.rs",
            "sed -i '' 's/a/b/g' -- src/a.rs",
            "sed -i '' 's/a/b/g' -src/a.rs",
            "sed -i '' 's/a/b/g' ~/a.rs",
            "sed -i '' 's/a/b/g' src/*.rs",
            "sed -i s/$X/b/g src/a.rs",
            "sed -i 's/a/b/g'x src/a.rs",
            "sed -E -i '' 's/a/b/g' src/a.rs",
            "sed -n -i '' 's/a/b/g' src/a.rs",
            "sed -i '' -e 's/a/b/g' -e 's/c/d/g' src/a.rs",
            "sed -i '' 's/a/b/g'",
            "X=1 sed -i '' 's/a/b/g' src/a.rs",
            "sudo sed -i '' 's/a/b/g' src/a.rs",
            "cd x && sed -i '' 's/a/b/g' a.rs",
            "sed -i '' 's/a/b/g' src",
            "sed -i '' 's/a/b/g' .",
        ] {
            assert!(
                rw(bad).is_none_or(|c| !c.starts_with("acus patch")),
                "{bad}"
            );
        }
        // Metacharacters that differ between sed and a literal search.
        assert_eq!(rw("sed -i '' 's/a*/b/g' src/a.rs"), None);
    }
}
