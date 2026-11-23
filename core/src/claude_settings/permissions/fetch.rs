// SPDX-License-Identifier: MPL-2.0
//! `WebFetch(domain:host)` rules.

/// Returns the lower-case host of `url`, without scheme, credentials, port or path.
pub(super) fn host(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    // The host follows the last `@`: `https://good.com@evil.com/` goes to evil.com.
    let hostport = authority.rsplit('@').next()?;
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        hostport.split(':').next()?
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Returns whether the rule's spec names `host`. `domain:example.com` covers the domain and, as an
/// approximation that errs towards refusing, its subdomains; `domain:*.example.com` covers the
/// subdomains only. A spec that is not `domain:...` covers nothing.
pub(super) fn matches(spec: &str, host: &str) -> bool {
    let Some(domain) = spec.strip_prefix("domain:") else {
        return false;
    };
    let domain = domain.trim().to_ascii_lowercase();
    if let Some(suffix) = domain.strip_prefix("*.") {
        return host.ends_with(&format!(".{suffix}"));
    }
    host == domain || host.ends_with(&format!(".{domain}"))
}
