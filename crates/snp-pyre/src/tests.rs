//! Crate-internal tests against the exhaustive CPython 3.12 tables (01 Addendum §A.8 item 3): Python's `\w`,
//! `\d`, `\s` per code point and the IGNORECASE literal sets, compared with the emitted classes and the folding
//! of `fold.rs`, with the UCD 15.0 → 16.0 delta allow-listed (§A.6.13 D2); and the `\N{…}` name tables.

use std::path::PathBuf;

use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, HirKind};
use serde_json::Value;

use crate::ast::Cat;
use crate::fold::RangeSet;
use crate::lower::category_text;

fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ranges(v: &Value) -> Vec<(u32, u32)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                u32::try_from(r[0].as_u64().unwrap()).unwrap(),
                u32::try_from(r[1].as_u64().unwrap()).unwrap(),
            )
        })
        .collect()
}

/// A class of scalar values (surrogates dropped).
fn class_of(rs: &[(u32, u32)]) -> ClassUnicode {
    let mut set = RangeSet::new(rs.to_vec());
    set.clip_surrogates();
    ClassUnicode::new(set.ranges().iter().map(|&(a, b)| {
        ClassUnicodeRange::new(char::from_u32(a).unwrap(), char::from_u32(b).unwrap())
    }))
}

fn emitted(text: &str) -> ClassUnicode {
    match regex_syntax::parse(&format!("[{text}]"))
        .unwrap()
        .into_kind()
    {
        HirKind::Class(Class::Unicode(c)) => c,
        other => panic!("{text}: {other:?}"),
    }
}

/// `a △ b` restricted to code points assigned in UCD 15.0 must be empty (D2 allows the rest).
fn assert_same_on_assigned(
    what: &str,
    mine: &ClassUnicode,
    python: &ClassUnicode,
    assigned: &ClassUnicode,
) {
    let mut only_mine = mine.clone();
    only_mine.difference(python);
    let mut only_python = python.clone();
    only_python.difference(mine);
    only_mine.union(&only_python);
    only_mine.intersect(assigned);
    assert!(
        only_mine.ranges().is_empty(),
        "{what}: differs from CPython 3.12 on assigned code points {:?}",
        only_mine.ranges()
    );
}

#[test]
fn categories_equal_python() {
    let v = load("classes.json");
    let assigned = class_of(&ranges(&v["assigned"]));
    for (key, cat, neg) in [
        ("word", Cat::Word, Cat::NotWord),
        ("digit", Cat::Digit, Cat::NotDigit),
        ("space", Cat::Space, Cat::NotSpace),
    ] {
        let python = class_of(&ranges(&v[key]));
        let mine = emitted(category_text(cat, false, false));
        assert_same_on_assigned(key, &mine, &python, &assigned);
        let mut python_neg = python;
        python_neg.negate();
        let mine_neg = emitted(category_text(neg, false, false));
        assert_same_on_assigned(key, &mine_neg, &python_neg, &assigned);
    }
    // `\s` is the same at any version (no new White_Space characters).
    assert_eq!(
        emitted(category_text(Cat::Space, false, false)),
        class_of(&ranges(&v["space"]))
    );
}

#[test]
fn ignorecase_literals_equal_python() {
    let v = load("classes.json");
    let assigned_ranges = ranges(&v["assigned"]);
    let assigned = RangeSet::new(assigned_ranges);
    let table = v["fold"].as_object().unwrap();
    let mut bad = Vec::new();
    for c in 0..=0x10_FFFF_u32 {
        if (0xD800..=0xDFFF).contains(&c) {
            continue;
        }
        let python: Vec<u32> = table.get(&format!("{c:x}")).map_or_else(
            || vec![c],
            |m| {
                m.as_array()
                    .unwrap()
                    .iter()
                    .map(|x| u32::try_from(x.as_u64().unwrap()).unwrap())
                    .collect()
            },
        );
        let mut mine = RangeSet::new(vec![(c, c)]);
        mine.fold_unicode();
        let mine: Vec<u32> = mine.ranges().iter().flat_map(|&(a, b)| a..=b).collect();
        if mine != python {
            // D2: allowed only when every differing code point (or `c` itself) is unassigned in UCD 15.0.
            let differing = mine
                .iter()
                .chain(&python)
                .filter(|x| !(mine.contains(x) && python.contains(x)));
            let explained =
                !assigned.contains(c) || differing.clone().all(|&x| !assigned.contains(x));
            if !explained {
                bad.push(format!("U+{c:04X}: snp {mine:X?}, CPython {python:X?}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} literal folding differences:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[cfg(feature = "py-re-names")]
mod names {
    use super::*;
    use crate::generated::names312::{ASSIGNED, CJK_UNIFIED};

    fn fnv(values: impl IntoIterator<Item = u32>) -> String {
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        for v in values {
            for b in v.to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x100_0000_01B3);
            }
        }
        format!("{h:016x}")
    }

    fn in_ranges(rs: &[(u32, u32)], c: u32) -> bool {
        rs.iter().any(|&(a, b)| (a..=b).contains(&c))
    }

    /// The non-algorithmic names `unicode_names2` gives the code points assigned in UCD 15.0 are exactly
    /// CPython 3.12's `unicodedata.name` (FNV digest over every (code point, name)).
    #[test]
    fn name_table_matches_python() {
        let v = load("names.json");
        let mut values = Vec::new();
        let mut count = 0_u64;
        for c in 0..=0x10_FFFF_u32 {
            let Some(ch) = char::from_u32(c) else {
                continue;
            };
            if !in_ranges(ASSIGNED, c)
                || in_ranges(CJK_UNIFIED, c)
                || (0xAC00..=0xD7A3).contains(&c)
            {
                continue;
            }
            let Some(name) = unicode_names2::name(ch).map(|n| n.to_string()) else {
                continue;
            };
            if name.starts_with("TANGUT IDEOGRAPH-") {
                continue;
            }
            count += 1;
            values.push(c);
            values.push(u32::try_from(name.chars().count()).unwrap());
            values.extend(name.chars().map(u32::from));
        }
        assert_eq!(count, v["count"].as_u64().unwrap());
        assert_eq!(fnv(values), v["digest"].as_str().unwrap());
    }

    /// `unicodedata.lookup` on every sampled spelling: names, lowercase/title/loose variants, aliases,
    /// Hangul and CJK algorithmic names, named sequences, UCD-16-only names.
    #[test]
    fn lookup_matches_python() {
        let v = load("names.json");
        let mut bad = Vec::new();
        for row in v["lookup"].as_array().unwrap() {
            let name = row[0].as_str().unwrap();
            let want = row[1].as_u64().map(|x| u32::try_from(x).unwrap());
            let got = crate::names::lookup(name).unwrap();
            if got != want {
                bad.push(format!("{name:?}: snp {got:X?}, CPython {want:X?}"));
            }
            // And through the parser (`\N{…}` in a str pattern).
            let pattern = format!("\\N{{{name}}}");
            if crate::validate(&pattern, crate::Mode::Str).is_ok() != want.is_some() {
                bad.push(format!("{pattern:?}: validate disagrees"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
