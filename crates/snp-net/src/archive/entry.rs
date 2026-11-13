//! Verified kernel (ADR-095): the archive entry-name check of ADR-077, called by `super::path::entry_path`.
//!
//! Spec and lemmas: `proofs/verus/spec/archive_entry.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md
//! §9.1 (K2). Plain Rust: the contract lives in `verus_keep_ghost` attributes and imports, which only Verus sets.

#[cfg(verus_keep_ghost)]
use crate::spec::archive_entry::{
    comps_w, done, has_nul, is_dotdot, is_kept, keep, lemma_done_prefix, lemma_keep_push,
    lemma_split_step, plain, plain_comp, refused, rooted, sep, spans_ok, spans_view, split_w,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Why an entry name is refused. The shell reports every refusal the same way (`NetError::UnsafeEntry`).
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Empty, absolute (`/x`, `\x`) or drive-prefixed (`C:…`).
    Rooted,
    /// A `..` component.
    DotDot,
    /// A NUL byte.
    Nul,
}

/// `b` separates components: `/`, and `\` too on Windows.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == sep(b, windows),
))]
fn is_sep(b: u8, windows: bool) -> bool {
    b == b'/' || (windows && b == b'\\')
}

/// `raw` starts like an absolute or drive-prefixed path on any OS: `/x`, `\x`, `C:…`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == rooted(raw@),
))]
#[expect(
    clippy::match_like_matches_macro,
    reason = "verus: kernels call no macro (G3) and vstd has no spec for u8::is_ascii_alphabetic; this match is proved equal to spec ascii_alpha"
)]
fn is_rooted(raw: &[u8]) -> bool {
    let Some(&c0) = raw.first() else {
        return false;
    };
    if c0 == b'/' || c0 == b'\\' {
        return true;
    }
    let Some(&c1) = raw.get(1) else {
        return false;
    };
    c1 == b':'
        && match c0 {
            b'A'..=b'Z' | b'a'..=b'z' => true,
            _ => false,
        }
}

/// The field `raw[start..end]`: `Ok(true)` keeps it, `Ok(false)` drops it (empty or `.`), `Err` refuses `..`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    requires
        start <= end <= raw.len(),
    ensures
        r matches Err(e) ==> e is DotDot,
        r is Err <==> is_dotdot(raw@.subrange(start as int, end as int)),
        r matches Ok(k) ==> k == is_kept(raw@.subrange(start as int, end as int)),
))]
#[expect(
    clippy::match_like_matches_macro,
    reason = "verus: kernels call no macro (G3), so `matches!` is spelled as a match"
)]
fn classify(raw: &[u8], start: usize, end: usize) -> Result<bool, Refusal> {
    let len = end.saturating_sub(start);
    let Some(&c0) = raw.get(start) else {
        return Ok(false);
    };
    if len == 0 {
        return Ok(false);
    }
    if len == 1 {
        return Ok(c0 != b'.');
    }
    let c1_dot = match raw.get(start.saturating_add(1)) {
        Some(&b'.') => true,
        _ => false,
    };
    if len == 2 && c0 == b'.' && c1_dot {
        return Err(Refusal::DotDot);
    }
    Ok(true)
}

/// The components of the archive entry name `raw` as `(start, end)` byte spans, in order: the name split on `/`
/// (and `\` when `windows`; the shell passes `cfg!(windows)`), with empty and `.` components dropped.
///
/// `Err` exactly when the name is empty, starts like an absolute or drive-prefixed path on any OS (`/x`, `\x`,
/// `C:…`), has a `..` component or contains a NUL byte (ADR-077; pre-commit 4.7.0 checks nothing,
/// `golang.py:108-110`, `ruby.py:81-84`). Every returned span is in bounds and spells a plain component (non-empty,
/// not `.` or `..`, no NUL, no separator), and the spans strictly increase. No strip count is taken, so a `..` in
/// what `--strip-components` later removes is refused too. Proved for every byte string and both values of
/// `windows` (P0-P3); with P5, appending the components to a normalised destination never leaves it lexically.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Err <==> refused(raw@, windows),
        r matches Err(e) ==> (e is Rooted <==> (raw.len() == 0 || rooted(raw@))),
        r matches Ok(v) ==> spans_ok(raw@, v@, windows),
))]
pub(crate) fn entry_components(raw: &[u8], windows: bool) -> Result<Vec<(usize, usize)>, Refusal> {
    if raw.is_empty() || is_rooted(raw) {
        return Err(Refusal::Rooted);
    }
    let n = raw.len();
    let mut comps: Vec<(usize, usize)> = Vec::new();
    let mut start: usize = 0;
    let mut i: usize = 0;
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(raw@.subrange(0, 0) =~= Seq::<u8>::empty());
        assert(split_w(raw@.subrange(0, 0), windows) == seq![Seq::<u8>::empty()]);
        assert(done(raw@, windows, 0) =~= Seq::<Seq<u8>>::empty());
        assert(spans_view(raw@, comps@) =~= keep(done(raw@, windows, 0)));
        assert(raw@.subrange(0, 0) =~= raw@.subrange(start as int, i as int));
    }
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            n == raw.len(),
            n > 0,
            !rooted(raw@),
            start <= i <= n,
            split_w(raw@.subrange(0, i as int), windows).len() >= 1,
            split_w(raw@.subrange(0, i as int), windows).last() == raw@.subrange(
                start as int,
                i as int,
            ),
            spans_view(raw@, comps@) == keep(done(raw@, windows, i as int)),
            forall|k: int|
                0 <= k < keep(done(raw@, windows, i as int)).len() ==> !is_dotdot(
                    #[trigger] keep(done(raw@, windows, i as int))[k],
                ),
            forall|j: int| 0 <= j < i ==> #[trigger] raw@[j] != 0u8,
            forall|j: int| start <= j < i ==> !sep(#[trigger] raw@[j], windows),
            forall|k: int| 0 <= k < comps@.len() ==> plain(raw@, #[trigger] comps@[k], windows),
            forall|k: int| 0 <= k < comps@.len() ==> #[trigger] comps@[k].1 < start,
            forall|k: int| 0 <= k < comps@.len() - 1 ==> #[trigger] comps@[k].1 < comps@[k + 1].0,
        decreases n - i,
    ))]
    while i < n {
        let Some(&b) = raw.get(i) else {
            return Err(Refusal::Nul);
        };
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_split_step(raw@, windows, i as int);
        }
        if b == 0 {
            return Err(Refusal::Nul);
        }
        if is_sep(b, windows) {
            match classify(raw, start, i) {
                Ok(true) => comps.push((start, i)),
                Ok(false) => {}
                Err(e) => {
                    #[cfg(verus_keep_ghost)]
                    proof! {
                        let f = raw@.subrange(start as int, i as int);
                        let d = done(raw@, windows, i as int);
                        let p = split_w(raw@.subrange(0, i as int), windows);
                        assert(p =~= d.push(f));
                        lemma_keep_push(d, f);
                        lemma_done_prefix(raw@, windows, i as int + 1);
                        let kd = keep(done(raw@, windows, i as int + 1));
                        assert(kd[kd.len() - 1] == f);
                        assert(comps_w(raw@, windows)[kd.len() - 1] == f);
                    }
                    return Err(e);
                }
            }
            #[cfg(verus_keep_ghost)]
            proof! {
                let f = raw@.subrange(start as int, i as int);
                let d = done(raw@, windows, i as int);
                let p = split_w(raw@.subrange(0, i as int), windows);
                assert(p =~= d.push(f));
                lemma_keep_push(d, f);
                assert(raw@.subrange(i as int + 1, i as int + 1) =~= Seq::<u8>::empty());
            }
            start = i.saturating_add(1);
        }
        i = i.saturating_add(1);
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(raw@.subrange(0, i as int).drop_last() =~= raw@.subrange(0, i as int - 1));
            assert(raw@.subrange(start as int, i as int) =~= raw@.subrange(start as int, i as int - 1).push(
                raw@[i as int - 1],
            ) || start == i);
            assert(spans_view(raw@, comps@) =~= keep(done(raw@, windows, i as int)));
        }
    }
    // The field still open at the end of the name.
    #[cfg(verus_keep_ghost)]
    proof! {
        let f = raw@.subrange(start as int, n as int);
        let d = done(raw@, windows, n as int);
        assert(raw@.subrange(0, n as int) =~= raw@);
        let p = split_w(raw@, windows);
        assert(p =~= d.push(f));
        lemma_keep_push(d, f);
    }
    match classify(raw, start, n) {
        Ok(true) => comps.push((start, n)),
        Ok(false) => {}
        Err(e) => {
            #[cfg(verus_keep_ghost)]
            proof! {
                let f = raw@.subrange(start as int, n as int);
                let c = comps_w(raw@, windows);
                assert(c[c.len() - 1] == f);
            }
            return Err(e);
        }
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        let c = comps_w(raw@, windows);
        assert(spans_view(raw@, comps@) =~= c);
        assert forall|k: int| 0 <= k < c.len() implies plain_comp(#[trigger] c[k], windows) by {
            assert(c[k] == spans_view(raw@, comps@)[k]);
            assert(plain(raw@, comps@[k], windows));
            let sp = comps@[k];
            assert forall|j: int| 0 <= j < c[k].len() implies #[trigger] c[k][j] != 0u8 && !sep(
                c[k][j],
                windows,
            ) by {
                assert(c[k][j] == raw@[sp.0 + j]);
            }
        }
        assert(!has_nul(raw@));
        assert(!refused(raw@, windows));
    }
    Ok(comps)
}
