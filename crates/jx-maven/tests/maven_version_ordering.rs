//! End-to-end checks ported from Apache Maven's own test suite
//! (`ComparableVersionTest.java`, maven-artifact). The tables below are
//! copied verbatim from that file so jx can't drift from Maven's ordering.

use jx_maven::ComparableVersion;
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn parse(version: &str) -> ComparableVersion {
    ComparableVersion::parse(version).unwrap_or_else(|e| panic!("cannot parse {version}: {e}"))
}

fn ordered(versions: &[&str]) {
    for i in 1..versions.len() {
        for j in i..versions.len() {
            let low = parse(versions[i - 1]);
            let high = parse(versions[j]);
            assert_lt(&low, &high, versions[i - 1], versions[j]);
        }
    }
}

fn assert_lt(low: &ComparableVersion, high: &ComparableVersion, low_text: &str, high_text: &str) {
    assert!(
        low.compare(high) == Ordering::Less,
        "expected {low_text} < {high_text}",
    );
    assert!(
        high.compare(low) == Ordering::Greater,
        "expected {high_text} > {low_text}",
    );
}

/// Java `checkVersionsHaveSameOrder`: ordering equality without structural
/// or hash equality (e.g. `1-ga0` orders as `1` but nests differently).
fn same_order(v1: &str, v2: &str) {
    let c1 = parse(v1);
    let c2 = parse(v2);
    assert_eq!(c1.compare(&c2), Ordering::Equal, "expected {v1} ~~ {v2}");
    assert_eq!(c2.compare(&c1), Ordering::Equal, "expected {v2} ~~ {v1}");
}

fn equal(v1: &str, v2: &str) {
    let c1 = parse(v1);
    let c2 = parse(v2);
    assert_eq!(c1.compare(&c2), Ordering::Equal, "expected {v1} == {v2}");
    assert_eq!(c2.compare(&c1), Ordering::Equal, "expected {v2} == {v1}");
    // java structural equals; ordering-equal wishes remain structural-distinct
    assert_eq!(c1, c2, "expected structural equality {v1} == {v2}");
    assert!(
        hash_value(&c1) == hash_value(&c2),
        "expected equal hashes for {v1} and {v2}",
    );
}

fn hash_value(version: &ComparableVersion) -> u64 {
    let mut hasher = DefaultHasher::new();
    version.canonical().hash(&mut hasher);
    version.canonical().hash(&mut hasher);
    hasher.finish()
}

/// Canonical form must be stable: `canonical(canonical(v)) == canonical(v)`.
fn stable_canonical(version: &str) {
    let c = parse(version);
    let once = c.canonical();
    let twice = parse(&once).canonical();
    assert_eq!(
        once, twice,
        "canonical {version} not stable: {once} / {twice}"
    );
}

#[test]
fn versions_qualifier_order() {
    ordered(&[
        "1-alpha2snapshot",
        "1-alpha2",
        "1-alpha-123",
        "1-beta-2",
        "1-beta123",
        "1-m2",
        "1-m11",
        "1-rc",
        "1-cr2",
        "1-rc123",
        "1-SNAPSHOT",
        "1",
        "1-sp",
        "1-sp2",
        "1-sp123",
        "1-abc",
        "1-def",
        "1-pom-1",
        "1-1-snapshot",
        "1-1",
        "1-2",
        "1-123",
    ]);
}

#[test]
fn versions_number_order() {
    ordered(&[
        "2.0", "2.0.a", "2-1", "2.0.2", "2.0.123", "2.1.0", "2.1-a", "2.1b", "2.1-c", "2.1-1",
        "2.1.0.1", "2.2", "2.123", "11.a2", "11.a11", "11.b2", "11.b11", "11.m2", "11.m11", "11",
        "11.a", "11b", "11c", "11m",
    ]);
}

#[test]
fn versions_equal() {
    equal("1", "1");
    equal("1", "1.0");
    equal("1", "1.0.0");
    equal("1.0", "1.0.0");
    equal("1", "1-0");
    equal("1", "1.0-0");
    equal("1.0", "1.0-0");
    // no separator between number and character
    equal("1a", "1-a");
    equal("1a", "1.0-a");
    equal("1a", "1.0.0-a");
    equal("1.0a", "1-a");
    equal("1.0.0a", "1-a");
    equal("1x", "1-x");
    equal("1x", "1.0-x");
    equal("1x", "1.0.0-x");
    equal("1.0x", "1-x");
    equal("1.0.0x", "1-x");
    equal("1cr", "1rc");
    // special "aliases" a, b and m
    equal("1a1", "1-alpha-1");
    equal("1b2", "1-beta-2");
    equal("1m3", "1-milestone-3");
    // case insensitive
    equal("1X", "1x");
    equal("1A", "1a");
    equal("1B", "1b");
    equal("1M", "1m");
    equal("1Cr", "1Rc");
    equal("1cR", "1rC");
    equal("1m3", "1Milestone3");
    equal("1m3", "1MileStone3");
    equal("1m3", "1MILESTONE3");
}

#[test]
fn versions_same_order_but_not_equal() {
    for v in [
        "1ga", "1release", "1final", "1Ga", "1GA", "1RELEASE", "1Final", "1FINAL",
    ] {
        assert_eq!(
            parse(v).compare(&parse("1")),
            Ordering::Equal,
            "expected {v} == 1"
        );
        // ordering alias, not structural equality
        assert_ne!(
            parse(v),
            parse("1"),
            "expected {v} structurally distinct from 1",
        );
    }
}

#[test]
fn release_qualifier_with_digit_ordering() {
    ordered(&[
        "1-rc1",
        "1",
        "1-ga1",
        "1-final2",
        "1-ga3",
        "1-release9",
        "1-ga11",
        "1-sp",
    ]);
    assert_eq!(parse("1-ga").compare(&parse("1")), Ordering::Equal);
    assert_eq!(parse("1-ga0").compare(&parse("1")), Ordering::Equal);
}

#[test]
fn version_comparing_pairs() {
    let pairs: &[(&str, &str)] = &[
        ("1", "2"),
        ("1.5", "2"),
        ("1", "2.5"),
        ("1.0", "1.1"),
        ("1.1", "1.2"),
        ("1.0.0", "1.1"),
        ("1.0.1", "1.1"),
        ("1.1", "1.2.0"),
        ("1.0-alpha-1", "1.0"),
        ("1.0-alpha-1", "1.0-alpha-2"),
        ("1.0-alpha-1", "1.0-beta-1"),
        ("1.0-beta-1", "1.0-SNAPSHOT"),
        ("1.0-SNAPSHOT", "1.0"),
        ("1.0-alpha-1-SNAPSHOT", "1.0-alpha-1"),
        ("1.0", "1.0-1"),
        ("1.0-1", "1.0-2"),
        ("1.0.0", "1.0-1"),
        ("2.0-1", "2.0.1"),
        ("2.0.1-klm", "2.0.1-lmn"),
        ("2.0.1", "2.0.1-xyz"),
        ("2.0.1", "2.0.1-123"),
        ("2.0.1-xyz", "2.0.1-123"),
    ];
    for (low, high) in pairs {
        assert_lt(&parse(low), &parse(high), low, high);
    }
}

#[test]
fn leading_zeroes_order() {
    ordered(&["0.7", "2"]);
    ordered(&["0.2", "1.0.7"]);
}

#[test]
fn mng6568() {
    ordered(&["0-alpha", "0", "x"]);
}

#[test]
fn mng5568() {
    let a = "6.1.0";
    let b = "6.1.0rc3";
    let c = "6.1H.5-beta";
    assert_lt(&parse(b), &parse(a), b, a);
    assert_lt(&parse(b), &parse(c), b, c);
    assert_lt(&parse(a), &parse(c), a, c);
}

#[test]
fn mng6572() {
    let a = "20190126.230843";
    let b = "1234567890.12345";
    let c = "123456789012345.1H.5-beta";
    let d = "12345678901234567890.1H.5-beta";
    assert_lt(&parse(a), &parse(b), a, b);
    assert_lt(&parse(b), &parse(c), b, c);
    assert_lt(&parse(a), &parse(c), a, c);
    assert_lt(&parse(c), &parse(d), c, d);
    assert_lt(&parse(b), &parse(d), b, d);
    assert_lt(&parse(a), &parse(d), a, d);
}

#[test]
fn release_zero_mixed_qualifier_transitivity() {
    ordered(&["1-0-ga0-alpha", "1-ga0", "1-ga.x.1"]);
}

#[test]
fn nested_qualifier_list_ordering() {
    ordered(&["1.alpha", "1", "1.ga.1"]);
    ordered(&["b.m.alpha", "b.m", "b.m.sp", "b.y"]);
    ordered(&["1-0-alpha1-beta", "1-alpha1", "1-alpha2"]);
    assert_eq!(parse("b.m").compare(&parse("b.m.final")), Ordering::Equal);
    assert_eq!(parse("b.y").compare(&parse("b.y.0")), Ordering::Equal);
}

#[test]
fn numeric_list_precedence_over_qualifiers() {
    ordered(&[
        "1-alpha",
        "1-alpha1",
        "1",
        "1-sp",
        "1-sp1",
        "1-x",
        "1-x1",
        "1-1",
        "1-1000000000",
        "1-1000000000000000000",
        "1.1",
    ]);
    ordered(&["1.0.x", "1-1"]);
    ordered(&["1.0.x", "1-1000000000"]);
    ordered(&["1.0.x", "1-1000000000000000000"]);
}

#[test]
fn zero_prefixed_release_qualifier_with_zero_digit() {
    for release in ["ga", "final", "release"] {
        same_order("1", &format!("1-{release}0"));
        same_order(&format!("1-{release}"), &format!("1-{release}0"));
        ordered(&["1", &format!("1-0-{release}.1"), &format!("1-{release}1")]);
        ordered(&[
            &format!("1-{release}0"),
            &format!("1-0-{release}.1"),
            &format!("1-{release}1"),
        ]);
    }
    assert_lt(&parse("1-alpha"), &parse("1-alpha0"), "1-alpha", "1-alpha0");
    assert_lt(&parse("1-x"), &parse("1-x0"), "1-x", "1-x0");
}

#[test]
fn qualifier_with_long_zero_digit() {
    for qualifier in ["alpha", "ga", "final", "release", "x"] {
        for zeros in [9usize, 10, 18, 19] {
            let zero = format!("1-{qualifier}{}", "0".repeat(zeros));
            equal(&format!("1-{qualifier}0"), &zero);
            let one = format!("1-{qualifier}1");
            assert_lt(&parse(&zero), &parse(&one), &zero, &one);
        }
    }
}

#[test]
fn zero_prefixed_qualifiers_are_equal() {
    for prefix in ["", "1-", "1.2-"] {
        for qualifier in [
            "alpha",
            "a1",
            "beta",
            "b2",
            "milestone",
            "m11",
            "rc",
            "cr1",
            "snapshot",
            "ga",
            "final",
            "release",
            "ga0",
            "ga1",
            "final0",
            "final2",
            "release0",
            "release9",
            "sp",
            "sp1",
            "x",
            "x1",
        ] {
            for zero in ["0-", "0.", "0.0-", "0-0-", "0-0.0-"] {
                let plain = format!("{prefix}{qualifier}");
                let prefixed = format!("{prefix}{zero}{qualifier}");
                equal(&plain, &prefixed);
            }
        }
    }
}

#[test]
fn mng6964() {
    let a = "1-0.alpha";
    let b = "1-0.beta";
    let c = "1";
    assert_lt(&parse(a), &parse(c), a, c);
    assert_lt(&parse(b), &parse(c), b, c);
    assert_lt(&parse(a), &parse(b), a, b);
}

#[test]
fn mng7644() {
    for x in [
        "abc",
        "alpha",
        "a",
        "beta",
        "b",
        "def",
        "milestone",
        "m",
        "RC",
    ] {
        assert_lt(
            &parse(&format!("1.0.0.{x}1")),
            &parse(&format!("1.0.0-{x}2")),
            "1.0.0.X1",
            "1.0.0-X2",
        );
        equal(&format!("2-{x}"), &format!("2.0.{x}"));
        equal(&format!("2-{x}"), &format!("2.0.0.{x}"));
        equal(&format!("2.0.{x}"), &format!("2.0.0.{x}"));
    }
}

#[test]
fn mng7714() {
    let f = parse("1.0.final-redhat");
    let sp1 = parse("1.0-sp1-redhat");
    let sp2 = parse("1.0-sp-1-redhat");
    let sp3 = parse("1.0-sp.1-redhat");
    assert_eq!(f.compare(&sp1), Ordering::Less);
    assert_eq!(f.compare(&sp2), Ordering::Less);
    assert_eq!(f.compare(&sp3), Ordering::Less);
}

#[test]
fn hyphenated_zero_canonical() {
    equal("1-0-1", "1--1");
    assert_eq!(parse("1-0-1").canonical(), "1-0-1");
    assert_eq!(parse("1--1").canonical(), "1-0-1");
    assert_eq!(parse("1-1").canonical(), "1-1");
    assert_lt(&parse("1-0-1"), &parse("1-1"), "1-0-1", "1-1");
    // MNG-6420 bug 2: 1-0-2 < 1-0.1 < 1-2
    assert_lt(&parse("1-0-2"), &parse("1-0.1"), "1-0-2", "1-0.1");
    assert_lt(&parse("1-0.1"), &parse("1-2"), "1-0.1", "1-2");
    assert_eq!(parse("1-0-2").canonical(), "1-0-2");
    equal("1-0-0-1", "1-0--1");
    equal("1-0-0-1", "1--0-1");
    assert_lt(&parse("1-0-0-1"), &parse("1-0-1"), "1-0-0-1", "1-0-1");
    assert_eq!(parse("1-0-0-1").canonical(), "1-0-0-1");
    equal("1-0-1-0", "1-0-1");
    assert_eq!(parse("1-0-1-0").canonical(), "1-0-1");
}

#[test]
fn canonical_stability() {
    for version in [
        "0.x", "0-x", "0.rc", "0-1", "0.2", "1.2.7", "1-ga", "1-sp", "1.0.x",
    ] {
        stable_canonical(version);
    }
    assert_eq!(parse("0.x").canonical(), "x");
    assert_eq!(parse("0.2").canonical(), "0.2");
}

#[test]
fn digits_outrank_letters() {
    assert_eq!(parse("7").compare(&parse("J")), Ordering::Greater);
    assert_eq!(parse("7").compare(&parse("c")), Ordering::Greater);
    assert_eq!(parse("1").compare(&parse("é")), Ordering::Greater);
    // non-BMP: mathematical digit used by the Java test
    let math_two = "\u{1D5C2}";
    assert_eq!(parse("1").compare(&parse(math_two)), Ordering::Greater);
}

#[test]
fn non_ascii_digits_are_not_digits() {
    let arabic_eight = "\u{0668}";
    // Maven treats non-ASCII digit characters as unknown qualifiers, which
    // rank below numbers (Java source: testNonAsciiDigits).
    assert_eq!(parse("1").compare(&parse(arabic_eight)), Ordering::Greater);
    assert_eq!(parse("9").compare(&parse(arabic_eight)), Ordering::Greater);
}

#[test]
fn lexicographic_unknown_qualifiers() {
    let aardvark = parse("aardvark");
    let zebra = parse("zebra");
    assert_eq!(zebra.compare(&aardvark), Ordering::Greater);
    let greek = parse("\u{03b6}\u{03ad}\u{03b2}\u{03c1}\u{03b1}"); // ζέβρα
    assert_eq!(greek.compare(&zebra), Ordering::Greater);
}

#[test]
fn leading_zero_runs_are_equal() {
    let lengths = (1..=19).rev();
    let ones: Vec<String> = lengths
        .clone()
        .map(
            |n| "1".to_string().repeat(n), /* actually '0'*n + '1' */
        )
        .collect();
    // build 0…01 forms of every length exactly like the Java test
    let zeros_then_one: Vec<String> = (1..=19)
        .rev()
        .map(|n: usize| "0".repeat(n.saturating_sub(1)) + "1")
        .collect();
    assert_eq!(ones.len(), zeros_then_one.len());
    // all compare (ordering) equal
    for text in &zeros_then_one {
        assert_eq!(parse(text).compare(&parse("1")), Ordering::Equal, "{text}");
    }
    let zero_versions: Vec<String> = (1..=19).rev().map(|n| "0".repeat(n)).collect();
    for text in &zero_versions {
        assert_eq!(parse(text).compare(&parse("0")), Ordering::Equal, "{text}");
    }
}

#[test]
fn version_length_bounded() {
    let deep = format!("1{}", "-1".repeat(127)); // 255 chars, deep nesting
    assert_eq!(parse(&deep).compare(&parse(&deep)), Ordering::Equal);
    assert_eq!(parse(&deep).compare(&parse("1")), Ordering::Greater);

    let deep_too_long = format!("1{}", "-1".repeat(150));
    assert!(ComparableVersion::parse(&deep_too_long).is_err());
    let digits_too_long = "1".repeat(257);
    assert!(ComparableVersion::parse(&digits_too_long).is_err());
}

#[test]
fn version_comparisons_form_a_total_order_on_sample() {
    // transitivity/antisymmetry/consistency sweep across the interesting set
    let mut versions = Vec::new();
    for prefix in ["", "0-", "0.", "0-0-", "1-", "1.0-", "1-0-", "1-0."] {
        for qualifier in [
            "alpha",
            "a1",
            "beta",
            "b2",
            "milestone",
            "m11",
            "rc",
            "cr1",
            "snapshot",
            "ga",
            "final",
            "release",
            "ga0",
            "ga1",
            "final0",
            "final2",
            "release0",
            "release9",
            "ga0000000000",
            "ga0000000000000000000",
            "sp",
            "sp1",
            "x",
            "x1",
        ] {
            versions.push(format!("{prefix}{qualifier}"));
        }
    }
    for extra in [
        "",
        "0",
        "1",
        "1-1",
        "1.1",
        "2",
        "999999999",
        "1000000000",
        "999999999999999999",
        "1000000000000000000",
        "1-ga.x.1",
        "1-0-ga0-alpha",
        "1-ga0",
        "1.ga.1",
        "1.alpha",
        "b.m.alpha",
        "b.m",
        "b.m.final",
        "b.m.sp",
        "b.y",
        "b.y.0",
        "1-0-alpha1-beta",
        "1-alpha1",
        "1-alpha2",
        "1-1000000000",
        "1-1000000000000000000",
        "1.0.x",
    ] {
        versions.push(extra.to_string());
    }
    let parsed: Vec<(String, ComparableVersion)> = versions
        .into_iter()
        .map(|v| {
            let parsed = parse(&v);
            (v, parsed)
        })
        .collect();

    for (ai, (a_text, a)) in parsed.iter().enumerate() {
        for (bi, (b_text, b)) in parsed.iter().enumerate() {
            let ab = a.compare(b);
            assert_eq!(
                ab,
                b.compare(a).reverse(),
                "antisymmetry: {a_text} / {b_text}"
            );
            if *a == *b {
                assert_eq!(
                    ab,
                    Ordering::Equal,
                    "structural equality: {a_text} / {b_text}"
                );
            }
            for (_c_text, c) in parsed.iter() {
                let bc = b.compare(c);
                let ac = a.compare(c);
                if ab != Ordering::Equal && bc != Ordering::Equal && ab == bc {
                    // a<b, b<c → a<c  (and symmetrically for >)
                    assert_eq!(
                        ab, ac,
                        "transitivity broken: {a_text} / {b_text} / {_c_text}"
                    );
                }
                if ab == Ordering::Equal && bc != Ordering::Equal {
                    assert_eq!(ac, bc, "consistency: {a_text} / {b_text} / {_c_text}");
                }
            }
            let _ = (ai, bi);
        }
    }
}
