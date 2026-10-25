// SPDX-License-Identifier: MPL-2.0
//! Turns a request into a plan: what the person is asked, how far an approval may reach and
//! which program runs.

use super::approval::{Gate, Remember};
use super::exec::Executor;
use super::git;
use super::request::{GhArgs, OpenUrlArgs, Refusal, Request, RunOutsideArgs};
use super::text::{MAX_FIELD, join_args};
use std::path::{Path, PathBuf};

/// Most arguments `gh` accepts in one request.
const MAX_GH_ARGS: usize = 64;

/// A validated request, ready for the question and then the run.
#[derive(Debug)]
pub struct Plan {
    pub tool: &'static str,
    pub gate: Gate,
    /// The repository root the approval is keyed by.
    pub repo: PathBuf,
    /// Labelled values for the dialog and the log.
    pub details: Vec<(&'static str, String)>,
    /// The program and arguments, run in the working directory.
    pub argv: Vec<String>,
}

/// Validates `request` for the working directory `cwd`. Asks git questions through `exec`.
pub fn plan(exec: &dyn Executor, cwd: &Path, request: &Request) -> Result<Plan, Refusal> {
    match request {
        Request::GitPush(args) => git::push_plan(exec, cwd, args),
        Request::Gh(args) => gh_plan(exec, cwd, args),
        Request::OpenUrl(args) => url_plan(exec, cwd, args),
        Request::RunOutside(args) => outside_plan(exec, cwd, args),
    }
}

fn too_long(what: &'static str, text: &str) -> Result<(), Refusal> {
    if text.chars().count() > MAX_FIELD {
        return Err(Refusal::TooLong(what, MAX_FIELD));
    }
    Ok(())
}

fn gh_plan(exec: &dyn Executor, cwd: &Path, args: &GhArgs) -> Result<Plan, Refusal> {
    if args.args.is_empty() {
        return Err(Refusal::NoGhArgs);
    }
    if args.args.len() > MAX_GH_ARGS {
        return Err(Refusal::TooLong("gh arguments", MAX_GH_ARGS));
    }
    let shown = join_args(&args.args);
    too_long("gh arguments", &shown)?;
    let read_only = is_read_only(&args.args);
    let gate = if read_only {
        Gate {
            class: "gh_read",
            remember: Remember::Repo,
        }
    } else {
        Gate {
            class: "gh_write",
            remember: Remember::Never,
        }
    };
    let kind = if read_only {
        "read-only"
    } else {
        "can change things"
    };
    Ok(Plan {
        tool: "gh",
        gate,
        repo: git::repo_root(exec, cwd),
        details: vec![
            ("Command", format!("gh {shown}")),
            ("Kind", kind.to_string()),
        ],
        argv: std::iter::once("gh".to_string())
            .chain(args.args.iter().cloned())
            .collect(),
    })
}

fn url_plan(exec: &dyn Executor, cwd: &Path, args: &OpenUrlArgs) -> Result<Plan, Refusal> {
    too_long("the URL", &args.url)?;
    if !is_web_url(&args.url) {
        return Err(Refusal::BadUrl);
    }
    Ok(Plan {
        tool: "open_url",
        gate: Gate {
            class: "open_url",
            remember: Remember::Session,
        },
        repo: git::repo_root(exec, cwd),
        details: vec![("URL", args.url.clone())],
        argv: vec!["open".to_string(), args.url.clone()],
    })
}

fn outside_plan(exec: &dyn Executor, cwd: &Path, args: &RunOutsideArgs) -> Result<Plan, Refusal> {
    too_long("the command", &args.command)?;
    if args.command.trim().is_empty() {
        return Err(Refusal::Empty("the command"));
    }
    Ok(Plan {
        tool: "run_outside",
        gate: Gate {
            class: "run_outside",
            remember: Remember::Never,
        },
        repo: git::repo_root(exec, cwd),
        details: vec![("Command", args.command.clone())],
        argv: vec![
            "/bin/zsh".to_string(),
            "-lc".to_string(),
            args.command.clone(),
        ],
    })
}

/// Returns whether `url` is an http or https URL with a host and no whitespace or control
/// characters.
fn is_web_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    rest.is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// `gh` subcommands that only read, as `(command, subcommands)`.
const READ_ONLY: &[(&str, &[&str])] = &[
    ("pr", &["view", "list", "diff", "checks", "status"]),
    ("run", &["view", "list", "watch"]),
    ("issue", &["view", "list"]),
    ("repo", &["view"]),
    ("release", &["view", "list"]),
];

/// Returns whether `gh args…` only reads. Everything not listed is a write.
pub fn is_read_only(args: &[String]) -> bool {
    match args {
        [first, rest @ ..] if first == "api" => api_is_read_only(rest),
        [first, second, ..] => READ_ONLY
            .iter()
            .any(|(command, subs)| command == first && subs.contains(&second.as_str())),
        _ => false,
    }
}

/// Returns whether `gh api` with `args` is a GET without a body. Any flag that could set a
/// method or a body, in any spelling, makes it a write; so does the `graphql` endpoint.
fn api_is_read_only(args: &[String]) -> bool {
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        let get = |method: &str| method.eq_ignore_ascii_case("GET");
        match arg {
            "-X" | "--method" => {
                if !args.next().is_some_and(get) {
                    return false;
                }
            }
            "--input" | "--field" | "--raw-field" | "graphql" => return false,
            _ if arg.starts_with("--method=") => {
                if !get(&arg["--method=".len()..]) {
                    return false;
                }
            }
            _ if arg.starts_with("--input=")
                || arg.starts_with("--field=")
                || arg.starts_with("--raw-field=") =>
            {
                return false;
            }
            _ if arg.starts_with("-X") && !arg.starts_with("--") => {
                if !get(&arg[2..]) {
                    return false;
                }
            }
            _ if arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.contains(['f', 'F', 'X']) =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_only(args: &str) -> bool {
        let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
        is_read_only(&args)
    }

    #[test]
    fn listed_subcommands_are_read_only() {
        for args in [
            "pr view 12",
            "pr list --state open",
            "pr diff",
            "pr checks",
            "pr status",
            "run view 9",
            "run list",
            "run watch 9",
            "issue view 3",
            "issue list",
            "repo view",
            "release view v1",
            "release list",
        ] {
            assert!(read_only(args), "{args}");
        }
    }

    #[test]
    fn everything_else_is_a_write() {
        for args in [
            "",
            "pr",
            "pr create --fill",
            "pr merge 12",
            "pr close 3",
            "issue create",
            "repo delete x",
            "release create v1",
            "run rerun 9",
            "auth token",
            "alias set x y",
            "extension exec x",
            "--repo a/b pr view 1",
            "PR view",
        ] {
            assert!(!read_only(args), "{args}");
        }
    }

    #[test]
    fn api_is_read_only_only_for_get_without_a_body() {
        for args in [
            "api repos/o/r",
            "api repos/o/r/pulls --paginate",
            "api -X GET repos/o/r",
            "api --method get repos/o/r",
            "api --method=GET repos/o/r",
            "api -XGET repos/o/r",
            "api repos/o/r -H Accept:application/json -q .name",
            "api repos/o/r -i",
        ] {
            assert!(read_only(args), "{args}");
        }
        for args in [
            "api -X POST repos/o/r/issues",
            "api --method DELETE repos/o/r",
            "api --method=PATCH repos/o/r",
            "api -XPOST repos/o/r",
            "api -X",
            "api repos/o/r -f title=x",
            "api repos/o/r -F body=@file",
            "api repos/o/r -ftitle=x",
            "api repos/o/r --field title=x",
            "api repos/o/r --field=title=x",
            "api repos/o/r --raw-field title=x",
            "api repos/o/r --raw-field=title=x",
            "api repos/o/r --input body.json",
            "api repos/o/r --input=body.json",
            "api graphql",
            "api -iX GET x",
        ] {
            assert!(!read_only(args), "{args}");
        }
    }

    #[test]
    fn only_web_urls_pass() {
        for url in ["http://a.b", "HTTPS://example.com/x?y=1#z"] {
            assert!(is_web_url(url), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://a.b",
            "https://",
            "https:///x",
            "https://a b",
            "https://a.b/\nx",
            "open -a Calculator",
            "",
        ] {
            assert!(!is_web_url(url), "{url}");
        }
    }
}
