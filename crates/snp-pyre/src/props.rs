//! Property tests (01 Addendum §A.8 item 5): `compile` never panics; the relaxed prefilter never rejects a
//! haystack the exact pattern matches, and the tier-1 one changes no answer (ADR-098); the fast tier-1 text
//! agrees with the exact text on every haystack that meets no condition bit; the cheap match kinds agree with
//! the engines.

use proptest::prelude::*;

use crate::compile::testing::{compile_engines, full_texts};
use crate::engine::{Hay, T1, T2, full, probe};
use crate::{Haystack, MatchKind, Mode, Options, PyRegex};

/// Pattern fragments covering the lowering rules (str and bytes).
const ATOMS: &[&str] = &[
    "a",
    "b",
    "K",
    "k",
    "s",
    "i",
    "I",
    "\u{e9}",
    "\u{df}",
    "\u{130}",
    "\u{131}",
    "\u{17f}",
    "\u{212a}",
    "1",
    "_",
    " ",
    "\\.",
    "\\-",
    "\\n",
    "\\x41",
    "\\0",
    ".",
    "[ab]",
    "[^a]",
    "[a-c\u{e9}]",
    "[^\\w]",
    "\\w",
    "\\W",
    "\\d",
    "\\D",
    "\\s",
    "\\S",
    "\\b",
    "\\B",
    "^",
    "$",
    "\\A",
    "\\Z",
    "(?:)",
    "()",
];

/// Wrappers: `{}` is the operand.
const WRAPS: &[&str] = &[
    "(?:{})",
    "({})",
    "{}*",
    "{}+",
    "{}?",
    "{}*?",
    "{}{2}",
    "{}{0,2}",
    "(?i:{})",
    "(?-i:{})",
    "(?s:{})",
    "(?m:{})",
    "(?a:{})",
    "(?x:{} )",
    "{}|a",
    "a|{}",
    "(?=a){}",
    "(?!a){}",
    "(?<=a){}",
    "(?<!b){}",
    "(?>{})",
    "{}*+",
    "{}?+",
    "(a){}\\1",
    "(a)?{}(?(1)b|c)",
    "(?P<n>a){}(?P=n)",
];

fn pattern() -> impl Strategy<Value = String> {
    let leaf = proptest::sample::select(ATOMS).prop_map(str::to_owned);
    leaf.prop_recursive(4, 24, 3, |inner| {
        prop_oneof![
            (proptest::sample::select(WRAPS), inner.clone()).prop_map(|(w, p)| w.replace("{}", &p)),
            (inner.clone(), inner).prop_map(|(a, b)| format!("{a}{b}")),
        ]
    })
}

fn haystack() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        proptest::sample::select(
            &[
                'a', 'b', 'K', 'k', '\u{212a}', '\u{e9}', '\u{df}', 'i', '\u{131}', '\n', ' ', '1',
                '_', '.',
            ][..],
        ),
        0..8,
    )
    .prop_map(|v| v.into_iter().collect())
}

fn options(mode: Mode, sel: u8) -> Options {
    let haystack = match sel % 3 {
        0 => Haystack::Path,
        1 => Haystack::Line,
        _ => Haystack::Blob,
    };
    Options {
        mode,
        haystack,
        ignore_case: sel & 4 != 0,
        multiline_dotall: sel & 8 != 0,
    }
}

fn line_shaped(h: &str) -> bool {
    !h.trim_end_matches('\n').contains('\n') && h.matches('\n').count() <= 1
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// No input string panics `validate` or `compile`, and validate and compile agree (compile adds only
    /// engine limits).
    #[test]
    fn compile_never_panics(src in ".{0,40}", sel in 0_u8..16, bytes in any::<bool>(), h in haystack()) {
        let mode = if bytes { Mode::Bytes } else { Mode::Str };
        let validated = crate::validate(&src, mode);
        let compiled = PyRegex::compile(&src, options(mode, sel));
        match (&validated, &compiled) {
            (Ok(_), Ok(_) | Err(crate::PyReError::Unsupported(_))) | (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "{src:?}: validate {validated:?} vs compile {:?}", compiled.as_ref().err()),
        }
        if let Ok(re) = compiled {
            let _results =
                (re.search(&h), re.match_prefix(&h), re.fullmatch(&h), re.search_bytes(h.as_bytes()));
        }
    }

    /// `L(P) ⊆ L(relaxed)` under search semantics, and `\A(?:P)\z` ⊆ `\A(?:relaxed)\z` (§A.6.11): the
    /// prefilter never rejects a real match. The reference is the exact text: tier 2's main text, tier 1's exact
    /// fallback (ADR-098).
    #[test]
    fn relaxed_is_a_superset(p in pattern(), sel in 0_u8..16, bytes in any::<bool>(), hs in proptest::collection::vec(haystack(), 1..12)) {
        let mode = if bytes { Mode::Bytes } else { Mode::Str };
        let opts = options(mode, sel);
        let Some(texts) = full_texts(&p, opts) else { return Ok(()) };
        let Some(relaxed) = texts.relaxed.as_deref() else { return Ok(()) };
        let reference = if texts.tier2 { Some(&*texts.main) } else { texts.exact.as_deref() };
        let Some(reference) = reference else { return Ok(()) };
        let depth = texts.depth.saturating_add(1);
        let built = (
            T2::build(reference, texts.bytes, depth),
            T1::build(relaxed, texts.bytes, depth),
            T2::build(&full(reference), texts.bytes, depth),
            T1::build(&full(relaxed), texts.bytes, depth),
        );
        let (Ok(exact), Ok(relaxed), Ok(exact_full), Ok(relaxed_full)) = built else {
            return Ok(());
        };
        for h in &hs {
            if opts.haystack == Haystack::Line && !line_shaped(h) {
                continue;
            }
            for hay in [Hay::Str(h), Hay::Bytes(h.as_bytes())] {
                if exact.is_match(hay) == Ok(true) || exact.matches_at_start(hay) == Ok(true) {
                    prop_assert!(relaxed.is_match(hay), "{p:?} {opts:?}: relaxed {:?} rejects {h:?}", texts.relaxed);
                }
                if exact_full.is_match(hay) == Ok(true) {
                    prop_assert!(relaxed_full.is_match(hay), "{p:?} {opts:?}: fullmatch relaxed {:?} rejects {h:?}", texts.relaxed);
                }
            }
        }
    }

    /// The tier-1 exact fallback answers the same with and without its prefilter (ADR-098), on haystacks that
    /// meet a condition bit (non-ASCII, empty, or any haystack for match spans): `re.search`, `re.match`,
    /// `re.fullmatch` and `search_bytes`.
    #[test]
    fn tier1_prefilter_keeps_every_answer(p in pattern(), sel in 0_u8..16, bytes in any::<bool>(), hs in proptest::collection::vec(haystack(), 1..12)) {
        let mode = if bytes { Mode::Bytes } else { Mode::Str };
        let opts = options(mode, sel);
        let Ok(re) = compile_engines(&p, opts) else { return Ok(()) };
        if re.kind() != MatchKind::Tier1 {
            return Ok(());
        }
        let answers = |h: &str| (re.search(h), re.match_prefix(h), re.fullmatch(h), re.search_bytes(h.as_bytes()));
        for h in &hs {
            if opts.haystack == Haystack::Line && !line_shaped(h) {
                continue;
            }
            let with = answers(h);
            probe::set_prefilter_off(true);
            let without = answers(h);
            probe::set_prefilter_off(false);
            prop_assert_eq!(with, without, "{:?} {:?} on {:?}", p, opts, h);
        }
    }

    /// The fast tier-1 text and the exact text agree on every haystack that meets no condition bit (§A.6.5).
    #[test]
    fn fast_agrees_with_exact_off_condition(p in pattern(), sel in 0_u8..16, bytes in any::<bool>(), hs in proptest::collection::vec(haystack(), 1..12)) {
        let mode = if bytes { Mode::Bytes } else { Mode::Str };
        let opts = options(mode, sel);
        let Some(texts) = full_texts(&p, opts) else { return Ok(()) };
        let Some(exact) = texts.exact.as_deref() else { return Ok(()) };
        let (Ok(fast), Ok(exact)) = (T1::build(&texts.main, texts.bytes, texts.depth), T2::build(exact, texts.bytes, texts.depth)) else {
            return Ok(());
        };
        for h in &hs {
            if texts.cond.needs_exact(h.as_bytes()) || (opts.haystack == Haystack::Line && !line_shaped(h)) {
                continue;
            }
            prop_assert_eq!(Ok(fast.is_match(Hay::Str(h))), exact.is_match(Hay::Str(h)), "{:?} {:?} on {:?}", p, opts, h);
        }
    }

    /// The cheap match kinds (ARCHITECTURE §3.3 item 4) agree with the engines.
    #[test]
    fn fast_kinds_agree_with_engines(
        lit in "[ab.\u{e9}\n]{0,3}",
        anchor in 0_u8..5,
        bytes in any::<bool>(),
        hs in proptest::collection::vec("[ab.\u{e9}\n]{0,5}", 1..12),
    ) {
        let escaped: String = lit.chars().map(|c| if c == '.' { "\\.".to_owned() } else if c == '\n' { "\\n".to_owned() } else { c.to_string() }).collect();
        let p = match anchor {
            0 => escaped,
            1 => format!("^{escaped}"),
            2 => format!("{escaped}$"),
            3 => format!("{escaped}\\Z"),
            _ => format!("\\A{escaped}"),
        };
        let mode = if bytes { Mode::Bytes } else { Mode::Str };
        let opts = Options { mode, ..Options::CONFIG };
        let cheap = PyRegex::compile(&p, opts).unwrap();
        prop_assert!(!matches!(cheap.kind(), MatchKind::Tier1 | MatchKind::Tier2), "{p:?} is not a cheap kind");
        let engines = compile_engines(&p, opts).unwrap();
        for h in &hs {
            prop_assert_eq!(cheap.search(h), engines.search(h), "search {:?} on {:?}", p, h);
            prop_assert_eq!(cheap.match_prefix(h), engines.match_prefix(h), "match {:?} on {:?}", p, h);
            prop_assert_eq!(cheap.fullmatch(h), engines.fullmatch(h), "fullmatch {:?} on {:?}", p, h);
            prop_assert_eq!(cheap.search_bytes(h.as_bytes()), engines.search_bytes(h.as_bytes()), "span {:?} on {:?}", p, h);
        }
    }
}

#[test]
#[expect(clippy::disallowed_methods, reason = "fixed repeat counts")]
fn deep_patterns_do_not_overflow_small_stacks() {
    // 484 nested groups (the CPython limit) parse, lower and build on a 256 KiB thread: the parser and the
    // lowering are iterative and deep engines are built on a helper thread.
    let handle = std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(|| {
            for p in [
                format!("{}a{}", "(".repeat(484), ")".repeat(484)),
                format!("{}a{}", "(?:".repeat(484), ")*".repeat(484)),
                format!("{}a{}", "(a|".repeat(484), ")".repeat(484)),
                format!("(a){}a{}", "(?(1)".repeat(969), ")".repeat(969)),
            ] {
                let re = PyRegex::compile(&p, Options::CONFIG);
                if let Ok(re) = re {
                    assert_eq!(re.search("a"), Ok(true), "{}", p.get(..20).unwrap_or(&p));
                    assert_eq!(re.fullmatch("a"), Ok(true));
                }
            }
        })
        .unwrap();
    handle.join().unwrap();
}
