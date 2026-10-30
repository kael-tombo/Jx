//! Shared did-you-mean diagnostics for key validation.
//!
//! The same small edit-distance picker backs unknown-key errors both in
//! project.toml (via jx-manifest) and in $JX_HOME/config.toml
//! (jx-common::config), so both report the same style: the unknown key,
//! a suggestion when one is close, and the list of allowed keys as the hint.

/// Simplest useful did-you-mean over the allowed key names.
pub fn closest<'a>(input: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let input = input.to_lowercase();
    candidates
        .iter()
        .copied()
        .filter(|c| c.len().abs_diff(input.len()) <= 3)
        .min_by_key(|c| {
            c.chars().zip(input.chars()).filter(|(a, b)| a != b).count()
                + c.len().abs_diff(input.len())
        })
}

/// An unknown-key error: message + optional suggestion + allowed keys hint.
pub fn unknown_key_error(
    kind: crate::error::Kind,
    kind_table: &str,
    key: &str,
    candidates: &[&str],
) -> crate::error::JxError {
    use std::fmt::Write as _;

    let mut message = format!("unknown key `{key}` in the `{kind_table}` table");
    if let Some(c) = closest(key, candidates) {
        let _ = write!(message, "\n       did you mean `{c}`?");
    }
    crate::error::JxError::new(kind, message).hint(format!(
        "allowed {kind_table} keys: {}",
        candidates.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Kind;

    #[test]
    fn closest_is_case_insensitive_and_edit_distance_based() {
        assert_eq!(
            closest("dependancies", &["dependencies"]),
            Some("dependencies")
        );
        assert_eq!(closest("z", &["repositories", "mirrors"]), None);
    }

    #[test]
    fn unknown_key_error_carries_suggestion_and_hint() {
        let e = unknown_key_error(Kind::Io, "repositories", "aut", &["url", "auth"]);
        assert!(e.message.contains("unknown key `aut`"), "{e}");
        assert!(e.message.contains("did you mean `auth`?"), "{e}");
        assert!(e.hint.as_deref().is_some_and(|h| h.contains("allowed")));
    }
}
