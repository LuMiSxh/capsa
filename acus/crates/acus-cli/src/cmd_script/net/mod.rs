// SPDX-License-Identifier: MPL-2.0
//! `fetch()` for `acus script`: a GET over http or https. Each redirect hop is checked against the
//! config's deny list and the Claude Code `WebFetch` rules. HTML comes back as markdown, other text
//! as it is.

use acus_script::HostError;
use capsa_core::claude_settings::permissions::Permissions;
use regex::Regex;
use std::time::{Duration, Instant};
use ureq::Body;
use ureq::http::Response;
use ureq::http::header::{CONTENT_TYPE, LOCATION};
use url::Url;

/// The longest one request may take, however much time the script has left.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The most redirects one fetch follows; the next one is refused.
const MAX_REDIRECTS: usize = 5;
/// The largest body read; a larger one is refused rather than cut short.
const MAX_BODY: u64 = 5 * 1024 * 1024;

/// The fetch settings from the config file.
#[derive(Default)]
pub(crate) struct FetchConfig {
    /// Hosts a fetch refuses: `host` exactly, or `*.host` for its subdomains.
    pub(crate) deny: Vec<String>,
}

/// What a script asks a fetch to keep of the page text.
#[derive(Default)]
pub(crate) struct FetchOpts {
    /// Keeps the lines that match this regex.
    pub(crate) grep: Option<String>,
    /// Keeps the first N lines, after `grep`.
    pub(crate) head: Option<usize>,
}

/// A fetched page: its status, the URL it ended at after redirects, and its text.
pub(crate) struct FetchResult {
    pub(crate) status: u16,
    pub(crate) url: String,
    pub(crate) text: String,
}

/// Fetches `url` and returns its text. Redirects count against `time_left` too; each request gets
/// at most 30 s of what is left.
pub(crate) fn fetch(
    perms: &Permissions,
    cfg: &FetchConfig,
    url: &str,
    opts: FetchOpts,
    time_left: Duration,
) -> Result<FetchResult, HostError> {
    let FetchOpts { grep, head } = opts;
    let grep = grep.as_deref().map(compile).transpose()?;
    let started = Instant::now();
    let mut url = Url::parse(url).map_err(|e| {
        HostError::new(format!("`{url}` is not a URL: {e}"))
            .with_hint("give an absolute URL such as https://example.com/page")
    })?;
    let mut hops = 0;
    let mut response = loop {
        check(perms, cfg, &url)?;
        let budget = time_left.saturating_sub(started.elapsed());
        let response = get(&url, budget)?;
        let Some(next) = redirect(&response, &url)? else {
            break response;
        };
        if hops == MAX_REDIRECTS {
            return Err(HostError::new(format!(
                "`{url}` redirects more than {MAX_REDIRECTS} times"
            ))
            .with_hint("fetch the final URL directly"));
        }
        hops += 1;
        url = next;
    };
    let status = response.status().as_u16();
    let text = read_text(&mut response, &url)?;
    Ok(FetchResult {
        status,
        url: url.to_string(),
        text: select(text, grep.as_ref(), head),
    })
}

/// Refuses a hop that is not http or https, whose host the deny list names, or that a `WebFetch`
/// rule covers.
fn check(perms: &Permissions, cfg: &FetchConfig, url: &Url) -> Result<(), HostError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HostError::new(format!(
            "cannot fetch `{url}`: only http and https URLs can be fetched"
        ))
        .with_hint("fetch an http:// or https:// URL"));
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if denied(&cfg.deny, &host) {
        return Err(HostError::new(format!(
            "fetching `{url}` is denied: its host `{host}` is in fetch_deny"
        ))
        .with_hint("remove the host from [script] fetch_deny in the config file to allow it"));
    }
    perms.check_fetch(url.as_str()).map_err(super::refused)
}

/// Returns whether a deny rule covers `host`: the rule `host` exactly, or `*.host` for any name
/// under it.
fn denied(deny: &[String], host: &str) -> bool {
    deny.iter().any(|rule| {
        let rule = rule.trim().trim_end_matches('.').to_ascii_lowercase();
        match rule.strip_prefix("*.") {
            Some(domain) => host.ends_with(&format!(".{domain}")),
            None => host == rule,
        }
    })
}

/// Sends one GET that does not follow redirects, with `budget` or the request timeout, whichever
/// is less.
fn get(url: &Url, budget: Duration) -> Result<Response<Body>, HostError> {
    if budget.is_zero() {
        return Err(timed_out(url));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("acus/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(budget.min(REQUEST_TIMEOUT)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .build()
        .into();
    agent.get(url.as_str()).call().map_err(|e| failure(url, e))
}

/// Returns the URL a redirect response points to, resolved against `from`; `None` for any other
/// response.
fn redirect(response: &Response<Body>, from: &Url) -> Result<Option<Url>, HostError> {
    if !response.status().is_redirection() {
        return Ok(None);
    }
    let Some(location) = response
        .headers()
        .get(LOCATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Ok(None);
    };
    from.join(location).map(Some).map_err(|e| {
        HostError::new(format!(
            "the redirect from `{from}` to `{location}` is not a URL: {e}"
        ))
    })
}

/// Reads the body, which must be under 5 MB, and returns its text: HTML as markdown, other text as
/// it is.
fn read_text(response: &mut Response<Body>, url: &Url) -> Result<String, HostError> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|e| failure(url, e))?;
    let mime = media_type(&content_type);
    if matches!(mime.as_str(), "text/html" | "application/xhtml+xml") {
        let html = String::from_utf8_lossy(&bytes);
        return htmd::convert(html.as_ref())
            .map_err(|e| HostError::new(format!("cannot convert the HTML of `{url}`: {e}")));
    }
    let untyped_text = mime.is_empty() && !bytes.contains(&0);
    if untyped_text || is_text(&mime) {
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    let kind = if mime.is_empty() {
        "none"
    } else {
        mime.as_str()
    };
    Err(
        HostError::new(format!("`{url}` is not text (content type: {kind})"))
            .with_hint("fetch() returns HTML and text; other file types cannot be fetched"),
    )
}

/// Returns the media type of a `Content-Type` value: lower-case, without its parameters.
fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Returns whether a media type is text that the script can read as it is.
fn is_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/javascript" | "application/xml"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}

/// Returns the error for a request or body that ureq could not finish.
fn failure(url: &Url, e: ureq::Error) -> HostError {
    match e {
        ureq::Error::Timeout(_) => timed_out(url),
        ureq::Error::BodyExceedsLimit(_) => {
            HostError::new(format!("the response from `{url}` is over 5 MB"))
                .with_hint("fetch a page under 5 MB")
        }
        e => HostError::new(format!("cannot fetch `{url}`: {e}"))
            .with_hint("check the URL and the network; the host may be down"),
    }
}

fn timed_out(url: &Url) -> HostError {
    HostError::new(format!("fetching `{url}` timed out")).with_hint(
        "the script's time limit ran out; raise [script] timeout in the config file, or fetch fewer pages",
    )
}

/// Returns the lines that `grep` accepts, then the first `head` of them. Text with neither option is
/// returned as it is.
fn select(text: String, grep: Option<&Regex>, head: Option<usize>) -> String {
    if grep.is_none() && head.is_none() {
        return text;
    }
    text.lines()
        .filter(|line| grep.is_none_or(|re| re.is_match(line)))
        .take(head.unwrap_or(usize::MAX))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compile(pattern: &str) -> Result<Regex, HostError> {
    Regex::new(pattern).map_err(|e| {
        HostError::new(format!("grep `{pattern}` is not a regex: {e}")).with_hint(
            "write a Rust regex; escape the characters of literal text that a regex reads as syntax",
        )
    })
}

#[cfg(test)]
mod tests;
