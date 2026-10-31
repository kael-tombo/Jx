//! Wanted → concrete version selection (ADR 0004 "version selection"):
//! `N`, `N.M`, `N.M.P` match *the highest stable release with that prefix*,
//! a leading `=` pins exactly, and pre-releases are never prefix candidates.
//! Ordering is the Maven port (`jx_maven::ComparableVersion`), so `1.10`
//! sorts above `1.9` like `mvn` says.

use jx_manifest::WantedVersion;
use jx_maven::ComparableVersion;
use jx_maven::version::is_pre_release;

/// Pick the concrete version for a wanted pin from the versions the
/// repositories list. `None` = nothing qualified; the caller turns that
/// into a diagnosed error (it knows the coordinate and the offering
/// context; see `concrete` in the graph module).
pub fn select_version(wanted: &WantedVersion, offered: &[String]) -> Option<String> {
    match wanted {
        // An exact pin is the plain string match: it must be listed as
        // written (a typo'd `=` pin fails here, not "somewhere nearby").
        WantedVersion::Exact(pin) => offered
            .iter()
            .find(|offered| offered.as_str() == pin)
            .cloned(),
        WantedVersion::Prefix(prefix) => highest_stable_with_prefix(prefix, offered),
    }
}

/// The highest offered version that extends `prefix` at a boundary, among
/// the *stable* ones. Boundary rule: the character after the prefix must be
/// a Maven separator (`.`, `-`, `_`, `+`) or end of string — `1` picks
/// `1.10` (".10" follows) but not `15.0`, and `1.1` picks `1.1.5` but not
/// `1.12` ("2" follows, not a separator).
fn highest_stable_with_prefix(prefix: &str, offered: &[String]) -> Option<String> {
    highest_stable(
        offered
            .iter()
            .filter(|candidate| extends_prefix(prefix, candidate)),
    )
}

/// The highest stable version among all offered entries; used by `jx add`
/// when the user names no version.
pub fn select_latest_stable(offered: &[String]) -> Option<String> {
    highest_stable(offered.iter())
}

fn highest_stable<'a>(candidates: impl Iterator<Item = &'a String>) -> Option<String> {
    candidates
        // Pre-releases are excluded even when they are the only qualifying
        // versions: round up to "no candidate" and let the caller's report
        // say so (ADR 0004: jitter like `5.0-M1` never ships to users).
        .filter(|candidate| !is_pre_release(candidate))
        .filter_map(|candidate| match ComparableVersion::parse(candidate) {
            // A listing entry past jx's 256-character bound is not
            // selectable — it cannot be ordered, so it cannot be highest.
            Ok(parsed) => Some((candidate, parsed)),
            Err(_) => None,
        })
        .max_by(|(_, a), (_, b)| a.compare(b))
        .map(|(candidate, _)| candidate.clone())
}

/// Whether `version` extends `prefix` at a Maven separator boundary.
fn extends_prefix(prefix: &str, version: &str) -> bool {
    let Some(rest) = version.strip_prefix(prefix) else {
        return false;
    };
    match rest.chars().next() {
        None => true,
        Some('.') | Some('-') | Some('_') | Some('+') => true,
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(list: &[&str]) -> Vec<String> {
        list.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn prefix_picks_the_highest_offered_release() {
        let versions = offered(&["1.0.0", "1.2.0", "1.9.1", "1.10.0", "2.0.0"]);
        let picked = select_version(&WantedVersion::Prefix("1".to_string()), &versions)
            .expect("candidates exist");
        assert_eq!(picked, "1.10.0", "Maven ordering: 1.10 > 1.9");
    }

    #[test]
    fn prefix_respects_the_separator_boundary() {
        let versions = offered(&["15.0", "1.10.0"]);
        let picked = select_version(&WantedVersion::Prefix("1".to_string()), &versions)
            .expect("1.10.0 qualifies");
        assert_eq!(picked, "1.10.0", "`1` must not expand to `15.0`");

        let none = select_version(&WantedVersion::Prefix("1".to_string()), &offered(&["15.0"]));
        assert!(none.is_none(), "`1` must not expand to `15.0`");

        let versions = offered(&["1.12.0", "1.1.5"]);
        let picked = select_version(&WantedVersion::Prefix("1.1".to_string()), &versions)
            .expect("candidates exist");
        assert_eq!(picked, "1.1.5", "`1.1` must not expand to `1.12`");
    }

    #[test]
    fn pre_releases_are_never_prefix_candidates() {
        let versions = offered(&["5.0.0-M1", "5.0.0-alpha1", "5.0.0-rc2"]);
        let none = select_version(&WantedVersion::Prefix("5".to_string()), &versions);
        assert!(none.is_none(), "milestones betas and rcs are no picks");

        let versions = offered(&["1.0", "1.0.0-beta1"]);
        let picked = select_version(&WantedVersion::Prefix("1".to_string()), &versions)
            .expect("a stable candidate exists");
        assert_eq!(picked, "1.0");
    }

    #[test]
    fn exact_pins_match_the_string_not_the_semantics() {
        let versions = offered(&["1.0.0", "1.0.0-jre"]);
        let picked = select_version(&WantedVersion::Exact("1.0.0-jre".to_string()), &versions)
            .expect("listed");
        assert_eq!(picked, "1.0.0-jre");

        let missing = select_version(&WantedVersion::Exact("2.0.16".to_string()), &versions);
        assert!(missing.is_none());
    }

    #[test]
    fn the_latest_stable_ignores_pre_releases() {
        let versions = offered(&["1.0", "2.0", "2.1-RC1", "2.1-M2"]);
        assert_eq!(select_latest_stable(&versions).as_deref(), Some("2.0"));
        assert!(select_latest_stable(&offered(&["2.1-RC1"])).is_none());
    }

    #[test]
    fn the_prefix_may_be_the_whole_version() {
        let versions = offered(&["33", "33.1"]);
        let picked = select_version(&WantedVersion::Prefix("33".to_string()), &versions)
            .expect("candidates exist");
        assert_eq!(picked, "33.1", "the listed `33` loses to `33.1`");
    }
}
