// SPDX-License-Identifier: MPL-2.0
//! Owns the file listings (`find -name`, `rg --files`, `fd`, the Glob tool) and the `acus files`
//! calls that replace them.

use super::*;

/// Returns the `-g` glob that lists the files a `find -name` pattern names. `acus files` matches
/// globs against the path below the directory it walks, and there `*` and `?` also match `/`, so
/// only two shapes mean the same as a match on the file name: a literal name (`**/NAME`) and a
/// suffix (`*SUFFIX`). Anything else is not converted. `fold` makes the letters of an ASCII
/// pattern match in both cases (`-iname`). `find` exits 0 when nothing matches and `acus files`
/// exits 1, so callers keep `find` as it is where `&&` or `||` follows.
fn name_glob(pat: &str, fold: bool) -> Option<String> {
    let pat = unquote(pat);
    let (star, lit) = pat
        .strip_prefix('*')
        .map_or((false, pat), |rest| (true, rest));
    if lit.is_empty()
        || lit.contains(['*', '?', '[', ']', '{', '}', '\\', '/', '!'])
        || lit.chars().any(char::is_control)
        || (fold && !lit.is_ascii())
    {
        return None;
    }
    let lit: String = lit
        .chars()
        .map(|c| {
            if fold && c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else {
                c.to_string()
            }
        })
        .collect();
    Some(if star {
        format!("*{lit}")
    } else {
        format!("**/{lit}")
    })
}

/// Reports whether `acus files` reads a path operand as a path and not as a flag.
fn path_operand(w: &str) -> Option<&str> {
    Some(unquote(w)).filter(|p| !p.is_empty() && !p.starts_with('-'))
}

fn files_call(paths: &[&str], flags: &[String]) -> String {
    let mut cmd = "acus files".to_owned();
    for p in paths {
        cmd += &format!(" {}", quote(p));
    }
    for f in flags {
        cmd += &format!(" {f}");
    }
    cmd
}

/// Converts `find [DIR…] [-type f] -name|-iname PAT`, which lists the files whose name matches.
/// Hidden and ignored files, which `find` lists, are skipped by `acus files`, which says so in its
/// output. Any other test or action (`-exec`, `-delete`, `-mtime`, `-size`, `-maxdepth`, `-o`,
/// `!`, a second name) is not converted.
pub(super) fn find_files(args: &[&str]) -> Option<String> {
    let split = args
        .iter()
        .position(|a| a.starts_with('-') || matches!(*a, "!" | "(" | ")"))
        .unwrap_or(args.len());
    let paths: Vec<&str> = args[..split]
        .iter()
        .map(|p| path_operand(p))
        .collect::<Option<_>>()?;
    let (mut glob, mut typed) = (None, false);
    let mut words = args[split..].iter().copied();
    while let Some(w) = words.next() {
        match w {
            "-type" if !typed && unquote(words.next()?) == "f" => typed = true,
            "-name" | "-iname" if glob.is_none() => {
                glob = Some(name_glob(words.next()?, w == "-iname")?);
            }
            _ => return None,
        }
    }
    Some(files_call(&paths, &[format!("-g {}", quote(&glob?))]))
}

/// Reports whether the segment is a `find` or `rg --files` listing.
pub(super) fn lists_files(seg: &str) -> bool {
    head(seg).is_some_and(|(h, mut a)| h == "find" || (h == "rg" && a.any(|w| w == "--files")))
}

/// Converts `rg --files [PATH…] [-g !NAME]… [--hidden] [--no-ignore|-u] [-L]`. An exclusion
/// (`!NAME`) is a name without `/` or brackets. An include `-g GLOB` is not converted, because
/// ripgrep lets it override ignore and hidden rules (`-g '*.log'` lists gitignored files), and
/// `-t TYPE` is not either, because ripgrep's type table is not the one `acus files` has (`-t c`
/// includes `.h`).
pub(super) fn rg_files(args: &[&str]) -> Option<String> {
    let (mut paths, mut flags, mut files) = (Vec::new(), Vec::new(), false);
    let mut words = args.iter().copied();
    while let Some(w) = words.next() {
        let (name, attached) = match w.strip_prefix("--") {
            Some(long) => long
                .split_once('=')
                .map_or((w, None), |(n, v)| (&w[..n.len() + 2], Some(v))),
            None if w.len() > 2 && matches!(&w[..2], "-g" | "-t") => (&w[..2], Some(&w[2..])),
            None => (w, None),
        };
        let mut value = || attached.or_else(|| words.next());
        match name {
            "--files" if attached.is_none() => files = true,
            "--hidden" if attached.is_none() => flags.push("--hidden".to_owned()),
            "--no-ignore" | "-u" if attached.is_none() => flags.push("-u".to_owned()),
            "-L" | "--follow" if attached.is_none() => flags.push("-L".to_owned()),
            "-g" | "--glob" => {
                let g = unquote(value()?);
                let g = match g.strip_prefix('!') {
                    Some(ex)
                        if !ex.is_empty() && !ex.contains(['/', '[', ']', '{', '}', '\\', '!']) =>
                    {
                        format!("!{ex}")
                    }
                    _ => return None,
                };
                flags.push(format!("-g {}", quote(&g)));
            }
            _ if !w.starts_with('-') => paths.push(path_operand(w)?),
            _ => return None,
        }
    }
    files.then(|| files_call(&paths, &flags))
}

/// Reports how `fd` matches a regex (case-insensitive unless the pattern has capitals) against
/// names and lists directories too: no glob means the same, so it gets a hint and no conversion.
pub(super) const FD_HINT: &str = "acus hint: `acus files [DIR] -g 'GLOB'` lists the files whose name matches a glob (sorted, .gitignore respected, hidden files skipped; `--hidden` and `-u` include them), and `acus find -l 'PAT'` lists the files containing a text.";
