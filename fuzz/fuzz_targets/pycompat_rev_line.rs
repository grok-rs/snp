//! Fuzz target `pycompat_rev_line`: autoupdate's `REV_LINE_RE` matcher (`autoupdate.py:116`, the
//! regex-free normative form of 15 §A4.6), the rev-line rewrite and migrate-config's header split.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks;
//! a match's groups rebuild the line (`g1 + 'rev:' + g2 + g3 + g4 + g5 + g6`, plus Python `$`'s
//! optional final `\n`); g1 and g4 are non-empty and g6 is `"\n"`.
//!
//! Owner: WP-11.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pycompat::{float, precommit, text};

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    for line in text::splitlines_keepends(&s).chain(std::iter::once(&*s)) {
        if let Some([g1, g2, g3, g4, g5, g6]) = precommit::rev_line_match(line) {
            let rebuilt = format!("{g1}rev:{g2}{g3}{g4}{g5}{g6}");
            assert!(
                line == rebuilt || line == format!("{rebuilt}\n"),
                "{line:?}"
            );
            assert!(!g1.is_empty() && !g4.is_empty() && g6 == "\n");
            assert!(g1.chars().all(snp_pycompat::chars::is_space));
            let out = precommit::rev_line_rewrite(line, "v2.0.0", Some("v2")).unwrap();
            assert!(out.ends_with("  # frozen: v2\n"));
            assert!(precommit::rev_line_rewrite(line, "v2", None).is_some());
        } else {
            assert!(precommit::rev_line_rewrite(line, "v2", None).is_none());
        }
    }
    let (head, rest) = precommit::migrate_header_split(&s);
    assert_eq!(head.len() + rest.len(), s.len());
    // float repr / duration of the first eight bytes as an IEEE-754 pattern
    if let Some(bits) = data.get(..8) {
        let x = f64::from_le_bytes(bits.try_into().unwrap());
        let r = float::repr(x);
        if x.is_finite() {
            assert_eq!(r.parse::<f64>().unwrap(), x, "{r}");
        }
        let _ = float::format_duration(x);
    }
});
