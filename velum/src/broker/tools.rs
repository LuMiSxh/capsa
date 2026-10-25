// SPDX-License-Identifier: MPL-2.0
//! Turns a request into a plan: what the person is asked, how far an approval may reach and
//! which program runs.

use super::approval::{Gate, Remember};
use super::exec::Executor;
use super::git;
use super::repo::Recorded;
use super::request::{GhArgs, OpenUrlArgs, Refusal, Request, RunOutsideArgs};
use super::text::{MAX_FIELD, forbidden, join_args};
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
    /// What else a remembered approval is bound to, such as the push URL.
    pub subject: String,
    /// Labelled values for the dialog and the log.
    pub details: Vec<(&'static str, String)>,
    /// The program and arguments, run in the working directory.
    pub argv: Vec<String>,
    /// Whether the run gets the git-safe environment (see `Executor::run`).
    pub git_safe: bool,
}

/// Validates `request` for the working directory `cwd`. `repo` is the repository recorded at
/// the start of the run, or why there is none. Asks git questions through `exec`.
pub fn plan(
    exec: &dyn Executor,
    cwd: &Path,
    repo: &Result<Recorded, String>,
    request: &Request,
) -> Result<Plan, Refusal> {
    let recorded = || -> Result<&Recorded, Refusal> {
        let recorded = repo.as_ref().map_err(|why| {
            Refusal::NoRepo(format!(
                "{why}. A repository made during this run cannot be used; \
                 push it from outside velum"
            ))
        })?;
        recorded.verify()?;
        Ok(recorded)
    };
    let plan = match request {
        Request::GitPush(args) => git::push_plan(exec, cwd, recorded()?, args)?,
        Request::Gh(args) => gh_plan(exec, cwd, recorded()?, args)?,
        Request::OpenUrl(args) => url_plan(cwd, args)?,
        Request::RunOutside(args) => outside_plan(cwd, args)?,
    };
    plain_text(&plan)?;
    Ok(plan)
}

/// Refuses a plan that shows or runs a control or invisible character, because what the
/// person reads would differ from what runs.
fn plain_text(plan: &Plan) -> Result<(), Refusal> {
    let shown = plan.details.iter().map(|(_, value)| value);
    let run = plan.argv.iter().skip(1);
    for value in shown.chain(run) {
        if let Some(found) = forbidden(value) {
            return Err(Refusal::Forbidden(plan.tool, u32::from(found)));
        }
    }
    Ok(())
}

fn too_long(what: &'static str, text: &str) -> Result<(), Refusal> {
    if text.chars().count() > MAX_FIELD {
        return Err(Refusal::TooLong(what, MAX_FIELD));
    }
    Ok(())
}

fn gh_plan(
    exec: &dyn Executor,
    cwd: &Path,
    repo: &Recorded,
    args: &GhArgs,
) -> Result<Plan, Refusal> {
    if args.args.is_empty() {
        return Err(Refusal::NoGhArgs);
    }
    if args.args.len() > MAX_GH_ARGS {
        return Err(Refusal::TooLong("gh arguments", MAX_GH_ARGS));
    }
    let shown = join_args(&args.args);
    too_long("gh arguments", &shown)?;
    let slug = if args.args[0] == "api" {
        git::push_url(exec, cwd, repo, "origin")
            .ok()
            .and_then(|url| github_slug(&url))
    } else {
        None
    };
    let read_only = is_read_only(&args.args, slug.as_deref());
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
        repo: repo.root.clone(),
        subject: String::new(),
        details: vec![
            ("Command", format!("gh {shown}")),
            ("Kind", kind.to_string()),
        ],
        argv: std::iter::once("gh".to_string())
            .chain(args.args.iter().cloned())
            .collect(),
        git_safe: true,
    })
}

fn url_plan(cwd: &Path, args: &OpenUrlArgs) -> Result<Plan, Refusal> {
    too_long("the URL", &args.url)?;
    if !is_web_url(&args.url) {
        return Err(Refusal::BadUrl);
    }
    Ok(Plan {
        tool: "open_url",
        gate: Gate {
            class: "open_url",
            remember: Remember::Never,
        },
        repo: cwd.to_path_buf(),
        subject: String::new(),
        details: vec![("URL", args.url.clone())],
        argv: vec!["open".to_string(), args.url.clone()],
        git_safe: false,
    })
}

fn outside_plan(cwd: &Path, args: &RunOutsideArgs) -> Result<Plan, Refusal> {
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
        repo: cwd.to_path_buf(),
        subject: String::new(),
        details: vec![("Command", args.command.clone())],
        argv: vec![
            "/bin/zsh".to_string(),
            "-lc".to_string(),
            args.command.clone(),
        ],
        git_safe: false,
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

/// Returns `owner/name`, lower-cased, of a github.com remote URL; anything else is `None`.
pub fn github_slug(url: &str) -> Option<String> {
    let path = [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
    ]
    .iter()
    .find_map(|prefix| url.strip_prefix(prefix))
    .or_else(|| {
        url.strip_prefix("https://")
            .and_then(|rest| rest.split_once('@'))
            .and_then(|(_, rest)| rest.strip_prefix("github.com/"))
    })?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let word = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (word(owner) && word(name)).then(|| format!("{owner}/{name}").to_ascii_lowercase())
}

/// `gh` subcommands that only read, as `(command, subcommands)`.
const READ_ONLY: &[(&str, &[&str])] = &[
    ("pr", &["view", "list", "diff", "checks", "status"]),
    ("run", &["view", "list", "watch"]),
    ("issue", &["view", "list"]),
    ("repo", &["view"]),
    ("release", &["view", "list"]),
];

/// Returns whether `arg` points `gh` at another repository or host, or opens a browser.
fn redirects(arg: &str) -> bool {
    let long = |name: &str| arg == name || arg.starts_with(&format!("{name}="));
    (arg.starts_with("-R") && !arg.starts_with("--"))
        || arg == "-w"
        || long("--repo")
        || long("--hostname")
        || long("--web")
        || arg.contains("://")
}

/// Returns whether `gh args…` only reads the repository of the working directory. `slug` is
/// `owner/name` of its github.com origin, if it has one. Everything not listed is a write.
pub fn is_read_only(args: &[String], slug: Option<&str>) -> bool {
    if args.iter().any(|arg| redirects(arg)) {
        return false;
    }
    match args {
        [first, rest @ ..] if first == "api" => {
            slug.is_some_and(|slug| api_is_read_only(rest, slug))
        }
        [first, second, rest @ ..] => {
            READ_ONLY
                .iter()
                .any(|(command, subs)| command == first && subs.contains(&second.as_str()))
                && (first != "repo" || names_no_repository(rest))
        }
        _ => false,
    }
}

/// Returns whether the arguments of `gh repo view` hold no repository name: every argument is
/// a flag or the value of `--json`, `--jq`, `--template` or `--branch`.
fn names_no_repository(args: &[String]) -> bool {
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg {
            "--json" | "-q" | "--jq" | "-t" | "--template" | "-b" | "--branch" => {
                args.next();
            }
            _ if arg.starts_with('-') => {}
            _ => return false,
        }
    }
    true
}

/// Flags of `gh api` that take a value in the next argument.
const API_VALUE_FLAGS: [&str; 9] = [
    "-H",
    "--header",
    "-q",
    "--jq",
    "-t",
    "--template",
    "--cache",
    "-p",
    "--preview",
];

/// Returns whether `endpoint` is a path under the repository `slug`, or the placeholders gh
/// fills from the working directory's repository.
fn endpoint_in_repo(endpoint: &str, slug: &str) -> bool {
    let path = endpoint.trim_start_matches('/').to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or("");
    if path.contains("graphql") || path.contains("..") || path.contains('%') {
        return false;
    }
    [format!("repos/{slug}"), "repos/{owner}/{repo}".to_string()]
        .iter()
        .any(|base| path == *base || path.starts_with(&format!("{base}/")))
}

/// Returns whether `gh api` with `args` is a GET without a body to one path of the repository
/// `slug`. Any flag that could set a method or a body, in any spelling, makes it a write.
fn api_is_read_only(args: &[String], slug: &str) -> bool {
    let mut args = args.iter().map(String::as_str);
    let mut endpoints = Vec::new();
    while let Some(arg) = args.next() {
        let get = |method: &str| method.eq_ignore_ascii_case("GET");
        match arg {
            "-X" | "--method" => {
                if !args.next().is_some_and(get) {
                    return false;
                }
            }
            "--input" | "--field" | "--raw-field" => return false,
            _ if API_VALUE_FLAGS.contains(&arg) => {
                args.next();
            }
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
            _ if arg.starts_with('-') => {}
            _ => endpoints.push(arg),
        }
    }
    matches!(endpoints.as_slice(), [endpoint] if endpoint_in_repo(endpoint, slug))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_only(args: &str) -> bool {
        let args: Vec<String> = args.split_whitespace().map(str::to_string).collect();
        is_read_only(&args, Some("o/r"))
    }

    #[test]
    fn gh_is_bound_to_the_repository_of_the_working_directory() {
        for args in [
            "pr view 1 --repo other/x",
            "pr view 1 --repo=other/x",
            "pr view 1 -R other/x",
            "pr view 1 -Rother/x",
            "pr view 1 -R=other/x",
            "pr list --hostname evil.example",
            "pr list --hostname=evil.example",
            "pr view 1 --web",
            "pr view 1 --web=true",
            "pr view 1 -w",
            "pr view https://github.com/other/x/pull/1",
            "repo view other/x",
            "api repos/other/x/pulls",
            "api /repos/other/x",
            "api orgs/o/members",
            "api user",
            "api repos/o/r2",
            "api repos/o/r/../../orgs/x",
            "api repos/o/r/%2e%2e/x",
            "api https://api.github.com/repos/o/r",
            "api repos/o/r repos/other/x",
            "api graphql",
            "api /graphql",
            "api graphql?x=1",
            "api GraphQL",
            "api repos/o/r/graphql",
            "api",
        ] {
            assert!(!read_only(args), "{args}");
        }
        for args in [
            "repo view",
            "repo view --json name",
            "api repos/o/r",
            "api /repos/O/R/pulls?state=open",
            "api repos/{owner}/{repo}/pulls",
            "api repos/o/r/contents/README.md -H Accept:x -q .sha",
        ] {
            assert!(read_only(args), "{args}");
        }
        let args: Vec<String> = ["api", "repos/o/r"].map(str::to_string).into();
        assert!(!is_read_only(&args, None));
        assert!(!is_read_only(&args, Some("other/x")));
    }

    #[test]
    fn github_slugs_come_only_from_github_dot_com() {
        for (url, want) in [
            ("git@github.com:Luca/Capsa.git", Some("luca/capsa")),
            ("https://github.com/o/r", Some("o/r")),
            ("https://github.com/o/r.git/", Some("o/r")),
            ("https://user@github.com/o/r.git", Some("o/r")),
            ("ssh://git@github.com/o/r.git", Some("o/r")),
            ("git@evil.example:o/r.git", None),
            ("https://github.com.evil.example/o/r", None),
            ("https://github.com/o", None),
            ("https://github.com/o/r/extra", None),
            ("", None),
        ] {
            assert_eq!(github_slug(url).as_deref(), want, "{url}");
        }
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
