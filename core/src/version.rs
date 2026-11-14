// SPDX-License-Identifier: MPL-2.0
//! Compares release versions by their semver core, where a prerelease is older than its release.

use std::cmp::Ordering;

/// The parts of a version that decide whether it is older: the `major.minor.patch` core, with
/// missing parts as 0, and whether a prerelease tag follows it.
#[derive(Debug, PartialEq, Eq)]
struct Semver {
    core: [u64; 3],
    prerelease: bool,
}

/// Parses `version` as semver core, ignoring build metadata after `+`. Two or three numeric
/// parts are accepted, so `2.1` and `2.1.0` are the same core; anything else is `None`.
fn semver(version: &str) -> Option<Semver> {
    let without_build = version.split('+').next()?;
    let (core_text, prerelease) = match without_build.split_once('-') {
        Some((core, _)) => (core, true),
        None => (without_build, false),
    };
    let parts = core_text
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    let [major, minor, patch] = match parts.as_slice() {
        [a, b] => [*a, *b, 0],
        [a, b, c] => [*a, *b, *c],
        _ => return None,
    };
    Some(Semver {
        core: [major, minor, patch],
        prerelease,
    })
}

/// Returns whether `version` has a core that [`is_older`] can compare.
pub fn is_version(version: &str) -> bool {
    semver(version).is_some()
}

/// Whether the version `a` is older than `b`. Cores compare numerically, so `0.106.0` is newer
/// than `0.85.1`; with equal cores a prerelease is older than the release, and two prereleases
/// are not ordered. A version that does not parse is never older, so it never warns.
pub fn is_older(a: &str, b: &str) -> bool {
    let (Some(a), Some(b)) = (semver(a), semver(b)) else {
        return false;
    };
    match a.core.cmp(&b.core) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => a.prerelease && !b.prerelease,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_older, is_version};

    #[test]
    fn a_prerelease_is_older_than_its_release_with_the_same_core() {
        assert!(is_older("1.0.0-beta", "1.0.0"));
        assert!(is_older("1.0.0-beta.1", "1.0.0"));
        assert!(!is_older("1.0.0", "1.0.0-beta.1"));
        assert!(!is_older("1.0.0-beta.1", "1.0.0-beta.2"));
    }

    #[test]
    fn cores_compare_numerically_and_missing_parts_are_zero() {
        assert!(is_older("0.85.1", "0.106.0"));
        assert!(!is_older("0.106.0", "0.85.1"));
        assert!(!is_older("2.1", "2.1.0"));
        assert!(!is_older("2.1.0", "2.1"));
        assert!(!is_older("0.85.1+build.7", "0.85.1"));
    }

    #[test]
    fn a_version_that_does_not_parse_is_never_older() {
        assert!(!is_older("latest", "1.0.0"));
        assert!(!is_older("1.0.0", "x"));
        assert!(is_version("2.1.311-beta.1"));
        assert!(!is_version("claude: not a version"));
    }
}
