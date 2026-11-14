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

/// ADR-098: the tier-1 exact fallback of a word-boundary pattern runs the backtracking engine only on the
/// non-ASCII haystacks its relaxed prefilter admits (B12u's `u05` ran it on all 20 000 paths before), with every
/// answer unchanged.
#[test]
fn tier1_prefilter_skips_the_backtracking_engine() {
    use crate::engine::probe;
    use crate::{MatchKind, Options, PyRegex};

    const EXTS: [&str; 20] = [
        "py", "md", "txt", "rs", "json", "yaml", "toml", "js", "ts", "css", "html", "c", "h", "go",
        "java", "rb", "sh", "ini", "cfg", "lock",
    ];
    let paths: Vec<String> = (0..2_000_usize)
        .map(|i| format!("café/über/données_{i}.{}", EXTS[i % EXTS.len()]))
        .collect();
    let re = PyRegex::compile(r"\b\w{3,8}\b/\w+\.md$", Options::CONFIG).unwrap();
    assert_eq!(re.kind(), MatchKind::Tier1);
    let answers = || -> Vec<bool> { paths.iter().map(|p| re.search(p).unwrap()).collect() };

    probe::set_prefilter_off(true);
    let before = probe::exact_runs();
    let reference = answers();
    let without = probe::exact_runs() - before;
    probe::set_prefilter_off(false);

    let before = probe::exact_runs();
    let forward_before = probe::forward_runs();
    let got = answers();
    let with = probe::exact_runs() - before;
    let forward = probe::forward_runs().since(forward_before);

    assert_eq!(got, reference);
    assert_eq!(reference.iter().filter(|&&m| m).count(), 100);
    assert_eq!(without, 2_000);
    // Only the `.md` paths reach the backtracking engine.
    assert_eq!(with, 100);
    // ADR-108: the prefilter's forward lazy DFA answers every path; neither the bounded backtracker nor the
    // PikeVM runs. (The relaxed text has an anchor, so no usefulness check runs on "".)
    assert_eq!(
        forward,
        probe::ForwardRuns {
            dfa: 2_000,
            backtrack: 0,
            pikevm: 0
        }
    );
}

/// ADR-108: B12u's `u05` relaxed text builds the prefilter's forward lazy DFA within the 256 KiB cap, which
/// `regex`'s engine could not use: its reverse DFA alone does not fit, so it ran the bounded backtracker on every
/// admitted path.
#[test]
fn u05_relaxed_text_builds_a_forward_dfa_at_the_cap() {
    use regex_automata::hybrid;
    use regex_automata::nfa::thompson::{self, WhichCaptures};

    use crate::Options;
    use crate::compile::testing::full_texts;
    use crate::engine::{PREFILTER_DFA_SIZE_LIMIT, Prefilter};

    let texts = full_texts(r"\b\w{3,8}\b/\w+\.md$", Options::CONFIG).unwrap();
    let relaxed = texts.relaxed.as_deref().unwrap();
    let prefilter = Prefilter::forward(
        relaxed,
        texts.bytes,
        texts.depth,
        texts.relaxed_anchored,
        PREFILTER_DFA_SIZE_LIMIT,
    )
    .unwrap();
    assert_eq!(prefilter.has_lazy_dfa(), Some(true));

    // Why `regex` fell back: the forward DFA fits the cap, the reverse one does not.
    let fits = |reverse: bool| {
        let nfa = thompson::Compiler::new()
            .configure(
                thompson::Config::new()
                    .reverse(reverse)
                    .which_captures(WhichCaptures::None),
            )
            .build(relaxed)
            .unwrap();
        hybrid::dfa::Builder::new()
            .configure(hybrid::dfa::Config::new().cache_capacity(PREFILTER_DFA_SIZE_LIMIT))
            .build_from_nfa(nfa)
            .is_ok()
    };
    assert!(fits(false));
    assert!(!fits(true));
}

/// ADR-108: a relaxed text whose forward lazy DFA does not fit the cap keeps its prefilter on the NFA engines
/// (never "no prefilter"): the bounded backtracker answers paths of at most 128 bytes, the `PikeVM` longer ones,
/// and every answer equals `regex`'s and the unfiltered exact engine's.
#[test]
#[expect(clippy::disallowed_methods, reason = "fixed repeat counts")]
fn a_forward_dfa_that_fails_at_the_cap_falls_back() {
    use crate::compile::testing::full_texts;
    use crate::engine::{Hay, PREFILTER_DFA_SIZE_LIMIT, Prefilter, T1, probe};
    use crate::{MatchKind, Options, PyRegex};

    let pattern = r"\b\w{40}\b";
    let texts = full_texts(pattern, Options::CONFIG).unwrap();
    let relaxed = texts.relaxed.as_deref().unwrap();
    let prefilter = Prefilter::forward(
        relaxed,
        texts.bytes,
        texts.depth,
        texts.relaxed_anchored,
        PREFILTER_DFA_SIZE_LIMIT,
    )
    .unwrap();
    assert_eq!(prefilter.has_lazy_dfa(), Some(false));
    let reference = T1::build(relaxed, texts.bytes, texts.depth).unwrap();

    let word = "\u{e9}".repeat(40);
    let short = [
        word.clone(),
        format!("x/{word}/y"),
        "\u{e9}".repeat(39),
        format!("{} {}", "\u{e9}".repeat(20), "\u{e9}".repeat(20)),
    ];
    let long = [
        format!("{}/{word}", "d/".repeat(70)),
        format!("{}/x", "\u{e9}/".repeat(80)),
    ];
    assert!(short.iter().all(|h| h.len() <= 128));
    assert!(long.iter().all(|h| h.len() > 128));

    let before = probe::forward_runs();
    for h in short.iter().chain(&long) {
        for hay in [Hay::Str(h), Hay::Bytes(h.as_bytes())] {
            assert_eq!(prefilter.admits(hay), reference.is_match(hay), "{h:?}");
        }
    }
    // Bytes that are not UTF-8 never match a str pattern, on either engine.
    let invalid = [word.as_bytes(), b"\xff"].concat();
    assert!(!prefilter.admits(Hay::Bytes(&invalid)));
    assert!(!reference.is_match(Hay::Bytes(&invalid)));
    let runs = probe::forward_runs().since(before);
    assert_eq!(
        runs,
        probe::ForwardRuns {
            dfa: 0,
            backtrack: 8,
            pikevm: 4
        }
    );

    // End to end: the same answers with and without the prefilter.
    let re = PyRegex::compile(pattern, Options::CONFIG).unwrap();
    assert_eq!(re.kind(), MatchKind::Tier1);
    let answers = |h: &str| {
        (
            re.search(h),
            re.match_prefix(h),
            re.fullmatch(h),
            re.search_bytes(h.as_bytes()),
        )
    };
    for h in short.iter().chain(&long) {
        let with = answers(h);
        probe::set_prefilter_off(true);
        let without = answers(h);
        probe::set_prefilter_off(false);
        assert_eq!(with, without, "{h:?}");
    }
}

/// ADR-108: when the forward lazy DFA quits (a Unicode word boundary meets a non-ASCII byte) or gives up (its
/// cache thrashes), the NFA engines answer, with `regex`'s answers.
#[test]
fn a_forward_dfa_that_quits_or_gives_up_falls_back() {
    use crate::engine::{Hay, PREFILTER_DFA_SIZE_LIMIT, Prefilter, T1, probe};

    let check = |text: &str, hays: &[&str], want: probe::ForwardRuns| {
        let prefilter =
            Prefilter::forward(text, false, 1, false, PREFILTER_DFA_SIZE_LIMIT).unwrap();
        assert_eq!(prefilter.has_lazy_dfa(), Some(true), "{text}");
        let reference = T1::build(text, false, 1).unwrap();
        let before = probe::forward_runs();
        for h in hays {
            assert_eq!(
                prefilter.admits(Hay::Str(h)),
                reference.is_match(Hay::Str(h)),
                "{text} on {h:?}"
            );
        }
        assert_eq!(probe::forward_runs().since(before), want, "{text}");
    };

    // Quit: a Unicode `\b` that must look at a non-ASCII byte (after `xyz`, or before it once the literal
    // prefilter jumps there); the ASCII haystack stays on the DFA.
    check(
        r"\bxyz\b",
        &["xyz\u{e9}", "\u{e9}xyz", "a xyz b"],
        probe::ForwardRuns {
            dfa: 1,
            backtrack: 2,
            pikevm: 0,
        },
    );

    // Give up: `[ab]*a[ab]{16}` has 2^17 DFA states; a pseudo-random a/b haystack makes about one new state per
    // byte, so the 256 KiB cache is cleared over and over at well under 10 bytes per state.
    let mut x = 0x2545_f491_u32;
    let random: String = (0..200_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if x & 1 == 0 { 'a' } else { 'b' }
        })
        .collect();
    let no_match = format!("{random}c");
    check(
        r"\A[ab]*a[ab]{16}c\z",
        &[&random, &no_match],
        probe::ForwardRuns {
            dfa: 0,
            backtrack: 0,
            pikevm: 2,
        },
    );
}
