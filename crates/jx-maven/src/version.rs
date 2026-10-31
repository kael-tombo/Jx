//! Maven `ComparableVersion` ordering, ported from Apache's implementation.
//!
//! Ported line-by-line from
//! <https://github.com/apache/maven/blob/master/compat/maven-artifact/src/main/java/org/apache/maven/artifact/versioning/ComparableVersion.java>
//! (read 2026-10-30). The ordering and equality assertions from Maven's own
//! `ComparableVersionTest.java` are re-exercised in
//! `crates/jx-maven/tests/maven_version_ordering.rs`.
//!
//! Kept exactly as Maven behaves: underscore is *not* a separator; a hyphen
//! followed by a digit in a nondigit run folds into a "combination" item
//! (`X-1` ≈ `X1`); numeric items rank above string items (`1.1 > 1-sp`,
//! `1.1 > 1-1`); unknown qualifiers sort after `sp`, lexicographically;                \
//! `ga`/`final`/`release` alias the empty qualifier for ordering but stay
//! structurally distinct items; nested-hyphen structure matters (`MNG-7644`:
//! `1.0.0.X1 < 1.0.0-X2`).
//!
//! jx deviation: Maven throws IllegalArgumentException for versions longer
//! than 256 characters; jx renders a diagnosed error instead.

use std::cmp::Ordering;

use jx_common::error::{JxError, Kind};

const MAX_VERSION_LENGTH: usize = 256;
const MAX_INT_ITEM_DIGITS: usize = 9;
const MAX_LONG_ITEM_DIGITS: usize = 18;

/// Known qualifiers, least to greatest; `""` is the release slot.
const QUALIFIERS: [&str; 7] = ["alpha", "beta", "milestone", "rc", "snapshot", "", "sp"];
/// `ga`/`final`/`release` alias the empty slot for ordering purposes.
const RELEASE_QUALIFIERS: [&str; 3] = ["ga", "final", "release"];
/// Rank string of the release qualifier: every qualifier is null-compared
/// against `StringItem.comparableQualifier("")`.
const RELEASE_RANK: &str = "5";

/// A parsed version identifier.
#[derive(Clone, Debug)]
pub struct ComparableVersion {
    items: Item,
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Int(i32),
    Long(i64),
    /// Zero-stripped unsigned decimal digits longer than 18 characters.
    BigInt(String),
    /// Lowercased qualifier value after alias expansion.
    Str(String),
    List(Vec<Item>),
    /// A letter run immediately followed by a digit run in one segment
    /// (`sp2`, `a1`): compared string part first, digit part second.
    Combination {
        string_part: String,
        digit_part: Box<Item>,
    },
}

impl ComparableVersion {
    /// Parse a version string; longer than `[MAX_VERSION_LENGTH]` chars is a
    /// rendered error (jx's replacement for Maven's IllegalArgumentException).
    pub fn parse(value: &str) -> Result<Self, JxError> {
        if value.chars().count() > MAX_VERSION_LENGTH {
            let head: String = value.chars().take(32).collect();
            return Err(JxError::new(
                Kind::Resolve,
                format!("version is longer than 256 characters: {head}…"),
            )
            .hint("check the version pin; jx refuses unbounded version strings"));
        }
        Ok(ComparableVersion {
            items: parse_items(value),
        })
    }

    /// Ordering comparison with Maven's exact semantics.
    pub fn compare(&self, other: &Self) -> Ordering {
        cmp_item(&self.items, Some(&other.items))
    }

    /// Maven's canonical form (`getCanonical`): hyphenated (nested) lists
    /// join with `-`, other components with `.`.
    pub fn canonical(&self) -> String {
        render(&self.items)
    }
}

impl PartialEq for ComparableVersion {
    /// Structural equality — the Java `equals`, not ordering equality (the
    /// ported suite asserts both, and they differ only for `ga`-aliases).
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
    }
}

fn render(item: &Item) -> String {
    match item {
        Item::Int(v) => v.to_string(),
        Item::Long(v) => v.to_string(),
        Item::BigInt(v) => v.clone(),
        Item::Str(v) => v.clone(),
        Item::Combination {
            string_part,
            digit_part,
        } => format!("{string_part}{}", render(digit_part)),
        Item::List(items) => {
            let mut out = String::new();
            for (i, child) in items.iter().enumerate() {
                if i > 0 {
                    out.push(if matches!(child, Item::List(_)) {
                        '-'
                    } else {
                        '.'
                    });
                }
                out.push_str(&render(child));
            }
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing — literal translation of ComparableVersion.parseVersion
// ---------------------------------------------------------------------------

fn parse_items(value: &str) -> Item {
    let lower: Vec<char> = value.chars().flat_map(char::to_lowercase).collect();
    let mut root: Vec<Item> = Vec::new();
    // Indices from the root down to the current list — the Rust equivalent of
    // the Java `list.add(list = new ListItem())` hand-off.
    let mut path: Vec<usize> = Vec::new();
    let mut is_digit = false;
    let mut is_combination = false;
    let mut start = 0usize;

    macro_rules! current_list {
        () => {{
            let mut cursor: &mut Vec<Item> = &mut root;
            for &p in &path {
                cursor = match &mut cursor[p] {
                    Item::List(v) => v,
                    _ => unreachable!("path indices always point at lists"),
                };
            }
            cursor
        }};
    }

    for i in 0..lower.len() {
        let c = lower[i];
        if c == '.' {
            if i == start {
                current_list!().push(Item::Int(0));
            } else {
                let buf: String = lower[start..i].iter().collect();
                let item = parse_item(&buf, is_combination, is_digit);
                current_list!().push(item);
            }
            is_combination = false;
            start = i + 1;
        } else if c == '-' {
            if i == start {
                current_list!().push(Item::Int(0));
            } else {
                // "X-1 is going to be treated as X1": fold buffers unless the
                // hyphen ends the string or starts a separator-only tail.
                if !is_digit && i != lower.len() - 1 && lower[i + 1].is_ascii_digit() {
                    is_combination = true;
                    continue;
                }
                let buf: String = lower[start..i].iter().collect();
                let item = parse_item(&buf, is_combination, is_digit);
                current_list!().push(item);
            }
            start = i + 1;
            let cur = current_list!();
            if !cur.is_empty() {
                cur.push(Item::List(Vec::new()));
                let idx = cur.len() - 1;
                path.push(idx);
            }
            is_combination = false;
        } else if c.is_ascii_digit() {
            if !is_digit && i > start {
                is_combination = true;
                let cur = current_list!();
                if !cur.is_empty() {
                    cur.push(Item::List(Vec::new()));
                    let idx = cur.len() - 1;
                    path.push(idx);
                }
            }
            is_digit = true;
        } else {
            if is_digit && i > start {
                let buf: String = lower[start..i].iter().collect();
                let item = parse_item(&buf, is_combination, true);
                current_list!().push(item);
                start = i;
                // The Java letter branch nests unconditionally after the
                // buffered run.
                current_list!().push(Item::List(Vec::new()));
                let idx = current_list!().len() - 1;
                path.push(idx);
                is_combination = false;
            }
            is_digit = false;
        }
    }

    if lower.len() > start {
        // "1.0.0.X1 < 1.0.0-X2 — treat .X as -X for any string qualifier X"
        if !is_digit {
            let cur = current_list!();
            if !cur.is_empty() {
                cur.push(Item::List(Vec::new()));
                let idx = cur.len() - 1;
                path.push(idx);
            }
        }
        let buf: String = lower[start..].iter().collect();
        let item = parse_item(&buf, is_combination, is_digit);
        current_list!().push(item);
    }

    // Java normalises every created list by popping its stack — newest
    // (deepest) first — so recurse into child lists first, then remove.
    fn normalize_all(list: &mut Vec<Item>) {
        for item in list.iter_mut() {
            if let Item::List(child) = item {
                normalize_all(child);
            }
        }
        normalize_removals(list);
    }
    normalize_all(&mut root);
    Item::List(root)
}

fn parse_item(buf: &str, is_combination: bool, is_digit: bool) -> Item {
    if is_combination {
        let folded = buf.replace('-', "");
        let index = folded
            .chars()
            .position(|c| c.is_ascii_digit())
            .unwrap_or(folded.len());
        let string_part = qualifier_value(&folded[..index], true);
        let digit_part = parse_item(&folded[index..], false, true);
        Item::Combination {
            string_part,
            digit_part: Box::new(digit_part),
        }
    } else if is_digit {
        let digits = strip_leading_zeroes(buf);
        if digits.len() <= MAX_INT_ITEM_DIGITS {
            Item::Int(digits.parse::<i32>().expect("digit run bounded"))
        } else if digits.len() <= MAX_LONG_ITEM_DIGITS {
            Item::Long(digits.parse::<i64>().expect("digit run bounded"))
        } else {
            Item::BigInt(digits)
        }
    } else {
        Item::Str(qualifier_value(buf, false))
    }
}

fn strip_leading_zeroes(buf: &str) -> String {
    let stripped = buf.trim_start_matches('0');
    if stripped.is_empty() {
        "0".to_string()
    } else {
        stripped.to_string()
    }
}

/// The StringItem constructor: single-letter `a`/`b`/`m` expand only when a
/// digit follows, and `cr` aliases `rc`.
fn qualifier_value(value: &str, followed_by_digit: bool) -> String {
    let value = if followed_by_digit && value.chars().count() == 1 {
        match value {
            "a" => "alpha",
            "b" => "beta",
            "m" => "milestone",
            other => return other.to_string(),
        }
        .to_string()
    } else {
        value.to_string()
    };
    if value == "cr" {
        "rc".to_string()
    } else {
        value
    }
}

// ---------------------------------------------------------------------------
// Null equivalence and normalisation (ListItem.normalize removals)
// ---------------------------------------------------------------------------

fn is_null(item: &Item) -> bool {
    match item {
        Item::Int(v) => *v == 0,
        Item::Long(v) => *v == 0,
        Item::BigInt(v) => v.bytes().all(|b| b == b'0'),
        Item::Str(v) => v.is_empty(),
        Item::List(v) => v.is_empty(),
        Item::Combination { .. } => false,
    }
}

/// Java `getType()`: STRING 1, LIST 2, numeric 3, COMBINATION 5.
fn item_type(item: &Item) -> u8 {
    match item {
        Item::Str(_) => 1,
        Item::List(_) => 2,
        Item::Int(_) | Item::Long(_) | Item::BigInt(_) => 3,
        Item::Combination { .. } => 5,
    }
}

fn normalize_removals(list: &mut Vec<Item>) {
    for i in (0..list.len()).rev() {
        if !is_null(&list[i]) {
            continue;
        }
        let i_is_last = i + 1 == list.len();
        let followed_by_string = !i_is_last && item_type(&list[i + 1]) == 1;
        let followed_by_comb_or_str_list = !i_is_last
            && match &list[i + 1] {
                Item::List(child) => matches!(
                    child.first(),
                    Some(Item::Combination { .. }) | Some(Item::Str(_))
                ),
                _ => false,
            };
        if i_is_last || followed_by_string || followed_by_comb_or_str_list {
            list.remove(i);
        }
    }
    // Removing a zero prefix must not leave an extra qualifier nesting level:
    // a list whose only item is another list adopts that list's items.
    if let [Item::List(child)] = list.as_mut_slice() {
        // Removing a zero prefix must not leave an extra qualifier nesting
        // level: a list whose only item is another list adopts its items.
        *list = std::mem::take(child);
    }
}

// ---------------------------------------------------------------------------
// Comparison — each Java compareTo ported as its own function
// ---------------------------------------------------------------------------

fn comparable_qualifier(qualifier: &str) -> String {
    if RELEASE_QUALIFIERS.contains(&qualifier) {
        return RELEASE_RANK.to_string();
    }
    match QUALIFIERS.iter().position(|q| *q == qualifier) {
        Some(index) => index.to_string(),
        None => format!("{}-{qualifier}", QUALIFIERS.len()),
    }
}

fn rank(qualifier: &str) -> String {
    comparable_qualifier(qualifier)
}

// (null_numeric folded into cmp_int/cmp_long/cmp_big; see those.)

/// `Item.compareTo(null)` for the string case.
fn str_vs_null(qualifier: &str) -> Ordering {
    rank(qualifier).as_str().cmp(RELEASE_RANK)
}

/// Exact unsigned decimal comparison for >18-digit runs.
fn compare_big_uints(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then(a.cmp(b))
}

/// `ListItem.compareTo(null)` — the MNG-6964 whole-list scan.
fn list_vs_null(list: &[Item]) -> Ordering {
    // MNG-6964: scan the whole list, not just the first element.
    for item in list {
        let result = cmp_item(item, None);
        if result != Ordering::Equal {
            return result;
        }
    }
    Ordering::Equal
}

fn cmp_item(item: &Item, other: Option<&Item>) -> Ordering {
    match item {
        Item::Int(v) => cmp_int(*v, other),
        Item::Long(v) => cmp_long(*v, other),
        Item::BigInt(v) => cmp_big(v, other),
        Item::Str(v) => cmp_str(v, other),
        Item::List(list) => cmp_list(list, other),
        Item::Combination {
            string_part,
            digit_part,
        } => cmp_comb(string_part, digit_part, other),
    }
}

fn cmp_int(value: i32, other: Option<&Item>) -> Ordering {
    match other {
        None => {
            if value == 0 {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        }
        Some(Item::Int(o)) => value.cmp(o),
        // LongItem/BigIntegerItem outrank IntItem by construction.
        Some(Item::Long(_) | Item::BigInt(_)) => Ordering::Less,
        // IntItem.compareTo(STRING/COMBINATION/LIST) → 1
        Some(Item::Str(_) | Item::Combination { .. } | Item::List(_)) => Ordering::Greater,
    }
}

fn cmp_long(value: i64, other: Option<&Item>) -> Ordering {
    match other {
        None => {
            if value == 0 {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        }
        Some(Item::Int(_)) => Ordering::Greater,
        Some(Item::Long(o)) => value.cmp(o),
        Some(Item::BigInt(_)) => Ordering::Less,
        Some(_) => Ordering::Greater,
    }
}

fn cmp_big(digits: &str, other: Option<&Item>) -> Ordering {
    match other {
        None => {
            if digits.bytes().all(|b| b == b'0') {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        }
        Some(Item::Int(_) | Item::Long(_)) => Ordering::Greater,
        Some(Item::BigInt(o)) => compare_big_uints(digits, o),
        Some(_) => Ordering::Greater,
    }
}

fn cmp_str(qualifier: &str, other: Option<&Item>) -> Ordering {
    match other {
        None => str_vs_null(qualifier),
        // "1.any < 1.1" — a string loses against any numeric.
        Some(Item::Int(_) | Item::Long(_) | Item::BigInt(_)) => Ordering::Less,
        Some(Item::Str(o)) => rank(qualifier).cmp(&rank(o)),
        Some(Item::Combination {
            string_part,
            digit_part,
        }) => {
            let result = rank(qualifier).cmp(&rank(string_part));
            if result != Ordering::Equal {
                result
            } else if rank(qualifier).as_str() == RELEASE_RANK {
                // A release-alias string tied with the combination's string
                // part: the digit decides the other way round
                // ("1-rc1 < 1, 1-ga1 > 1" asymmetry lives here).
                cmp_item(digit_part, None).reverse()
            } else {
                Ordering::Less
            }
        }
        Some(Item::List(_)) => {
            // StringItem.compareTo(LIST_ITEM): -item.compareTo(this)
            let string_item = Item::Str(qualifier.to_string());
            match other {
                Some(Item::List(list)) => cmp_list(list, Some(&string_item)).reverse(),
                _ => unreachable!(),
            }
        }
    }
}

fn cmp_list(list: &[Item], other: Option<&Item>) -> Ordering {
    match other {
        None => {
            if list.is_empty() {
                Ordering::Equal
            } else {
                list_vs_null(list)
            }
        }
        // "1-1 < 1.0.x" — numeric positions outrank nested hyphen lists.
        Some(Item::Int(_) | Item::Long(_) | Item::BigInt(_)) => Ordering::Less,
        Some(Item::Str(_) | Item::Combination { .. }) => {
            let mut scalar = if list.is_empty() {
                cmp_item(other.unwrap(), None).reverse()
            } else {
                cmp_item(&list[0], other)
            };
            if scalar != Ordering::Equal {
                return scalar;
            }
            for element in list.iter().skip(1) {
                scalar = cmp_item(element, None);
                if scalar != Ordering::Equal {
                    return scalar;
                }
            }
            scalar
        }
        Some(Item::List(rhs)) => {
            // Java's while loop keeps pairing to exhaustion of BOTH sides;
            // a null side compares against null only until one item decides.
            let count = list.len().max(rhs.len());
            for index in 0..count {
                let result = match (list.get(index), rhs.get(index)) {
                    (Some(left), Some(right)) => cmp_item(left, Some(right)),
                    (None, Some(right)) => cmp_item(right, None).reverse(),
                    (Some(left), None) => cmp_item(left, None),
                    (None, None) => Ordering::Equal,
                };
                if result != Ordering::Equal {
                    return result;
                }
            }
            Ordering::Equal
        }
    }
}

fn cmp_comb(string_part: &str, digit_part: &Item, other: Option<&Item>) -> Ordering {
    match other {
        None => {
            // "1-rc1 < 1, 1-ga1 > 1"
            let result = str_vs_null(string_part);
            if result == Ordering::Equal {
                cmp_item(digit_part, None)
            } else {
                result
            }
        }
        Some(Item::Int(_) | Item::Long(_) | Item::BigInt(_)) => Ordering::Less,
        Some(Item::Str(_)) => {
            let string_item = Item::Str(string_part.to_string());
            let result = cmp_item(&string_item, other);
            if result != Ordering::Equal {
                result
            } else if str_vs_null(string_part) == Ordering::Equal {
                // Release alias tied: the digit decides ("1-ga1 > 1").
                cmp_item(digit_part, None)
            } else {
                Ordering::Greater
            }
        }
        Some(Item::List(_)) => {
            // CombinationItem.compareTo(LIST_ITEM): -item.compareTo(this)
            let comb = Item::Combination {
                string_part: string_part.to_string(),
                digit_part: Box::new(digit_part.clone()),
            };
            match other {
                Some(Item::List(list)) => cmp_list(list, Some(&comb)).reverse(),
                _ => unreachable!(),
            }
        }
        Some(Item::Combination {
            string_part: other_string,
            digit_part: other_digit,
        }) => {
            let result = rank(string_part).cmp(&rank(other_string));
            if result == Ordering::Equal {
                cmp_item(digit_part, Some(other_digit))
            } else {
                result
            }
        }
    }
}

/// Qualifier spellings jx classifies as *pre-releases* (never stable picks,
/// ADR 0004). Long forms from Maven's ordering table, plus the short
/// spellings that convention attaches to them (`1.0-M1`, `1.0-b01`) and the
/// ones the ecosystem routinely publishes (`cr`, `ea`).
const PRE_RELEASE_QUALIFIERS: [&str; 10] = [
    "alpha",
    "a",
    "beta",
    "b",
    "milestone",
    "m",
    "rc",
    "cr",
    "snapshot",
    "ea",
];

/// Whether a version reads as a pre-release: any Maven separator segment
/// (`.`, `-`, `_`, `+`) whose leading letter run is a pre-release qualifier
/// (`alpha1`, `b02`, `M3` segments down to `m`).
///
/// Text-level, deliberately: `ComparableVersion` ranks unknown qualifiers
/// *above* the stable slot, so relying on the parse alone would let e.g.
/// `5.0-M3` beat `5.0` in a pick — the check is about stability, not order.
pub fn is_pre_release(version: &str) -> bool {
    version
        .to_lowercase()
        .split(['.', '-', '_', '+'])
        .any(|segment| {
            let letters: String = segment
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect();
            PRE_RELEASE_QUALIFIERS.contains(&letters.as_str())
        })
}

#[cfg(test)]
mod pre_release_tests {
    use super::is_pre_release;

    #[test]
    fn releases_and_quality_markers_stay_stable() {
        for version in [
            "1.0.0",
            "33.4.8-jre",
            "25.0.1+9",
            "2.17.292",
            "1.0.0.Final",
            "1.0.0-GA",
            "5.2.1.RELEASE",
            "1.0-android",
        ] {
            assert!(
                !is_pre_release(version),
                "{version} wrongly classified as a pre-release"
            );
        }
    }

    #[test]
    fn pre_release_spellings_are_flagged() {
        for version in [
            "1.0-alpha",
            "2.0.0-alpha1",
            "1.0-b01",
            "5.0-M1",
            "5.2.1-m3",
            "2.0.0-rc2",
            "1.0-cr1",
            "3.0.0_EA",
            "1.0-snapshot",
            "7.0.0-beta",
        ] {
            assert!(is_pre_release(version), "{version} should be a pre-release");
        }
    }

    #[test]
    fn continuation_words_are_not_qualifiers() {
        // A letter run that merely *starts* like a qualifier is not one:
        // only whole leading runs match.
        for version in ["1.0-betamax", "1.0-released", "1.0-access"] {
            assert!(
                !is_pre_release(version),
                "{version} wrongly classified as a pre-release"
            );
        }
    }
}
