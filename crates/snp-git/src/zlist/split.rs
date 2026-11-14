//! Verified kernel (ADR-095): the spans of `ZList::from_z`/`from_z_dedup` (`git.zsplit`, `git.py:19-24`), called by
//! `super::ZList::split`.
//!
//! Spec and lemmas: `proofs/verus/spec/zlist_split.rs` and `proofs/verus/spec/zsplit.rs`. Guarantees and
//! non-guarantees: docs/VERIFICATION.md §9.2 (K5b). Plain Rust: the contract lives in `verus_keep_ghost` attributes
//! and imports, which only Verus sets. The byte search is injected through [`FindByte`] (ADR-095 §2.6): the
//! production finder is memchr (`super::Memchr`, a trusted item of `proofs/registry.toml`), checked against the
//! verified [`Scalar`] by a differential test.
#[cfg(verus_keep_ghost)]
use crate::spec::zlist_split::{dedup_adjacent, spans_ok, spans_view};
#[cfg(verus_keep_ghost)]
use crate::spec::zsplit::{
    first_from, lemma_split_win_shift, lemma_split_win_skip, lstrip0, nul_free, rstrip0, split_win,
    strip0, zsplit_spec,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// A byte search (kernel trait, ADR-095 §2.6).
pub(crate) trait FindByte {
    /// The first index `i >= from` with `hay[i] == needle`, or `None` when no byte at or after `from` is `needle`
    /// (also when `from >= hay.len()`).
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            first_from(hay@, needle, from as int, r),
    ))]
    fn find_byte(&self, needle: u8, hay: &[u8], from: usize) -> Option<usize>;
}

/// The scalar reference [`FindByte`], verified against the trait's contract.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
pub(crate) struct Scalar;

impl FindByte for Scalar {
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            first_from(hay@, needle, from as int, r),
    ))]
    fn find_byte(&self, needle: u8, hay: &[u8], from: usize) -> Option<usize> {
        let mut i = from;
        #[cfg_attr(verus_keep_ghost, verus_spec(
            invariant
                from <= i,
                forall|j: int| from <= j < i ==> hay@[j] != needle,
            decreases hay.len() - i,
        ))]
        while i < hay.len() {
            if hay.get(i) == Some(&needle) {
                return Some(i);
            }
            i = i.saturating_add(1);
        }
        None
    }
}

/// `buf[xs..xe] == buf[ys..ye]`.
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "verus: the indices stay below both ends and the offsets cannot overflow (ADR-095)"
)]
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    requires
        xs <= xe <= buf.len(),
        ys <= ye <= buf.len(),
    ensures
        r == (buf@.subrange(xs as int, xe as int) == buf@.subrange(ys as int, ye as int)),
))]
fn same(buf: &[u8], xs: usize, xe: usize, ys: usize, ye: usize) -> bool {
    if xe - xs != ye - ys {
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(buf@.subrange(xs as int, xe as int).len() != buf@.subrange(ys as int, ye as int).len());
        }
        return false;
    }
    let len = xe - xs;
    let mut i: usize = 0;
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(buf@.subrange(xs as int, xs as int) =~= buf@.subrange(ys as int, ys as int));
    }
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            xs <= xe <= buf.len(),
            ys <= ye <= buf.len(),
            len == xe - xs == ye - ys,
            i <= len,
            buf@.subrange(xs as int, xs + i) == buf@.subrange(ys as int, ys + i),
        decreases len - i,
    ))]
    while i < len {
        if buf[xs + i] != buf[ys + i] {
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(buf@.subrange(xs as int, xe as int)[i as int] != buf@.subrange(
                    ys as int,
                    ye as int,
                )[i as int]);
            }
            return false;
        }
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(buf@.subrange(xs as int, xs + i + 1) =~= buf@.subrange(xs as int, xs + i).push(
                buf@[xs + i],
            ));
            assert(buf@.subrange(ys as int, ys + i + 1) =~= buf@.subrange(ys as int, ys + i).push(
                buf@[ys + i],
            ));
        }
        i += 1;
    }
    true
}

/// The spans of `git.zsplit(buf)` (`git.py:19-24`): leading and trailing NULs stripped, then one `(start, end)`
/// per NUL-separated record, interior empty records kept; with `dedup`, a record equal to the last kept record is
/// skipped (`ls-files --deduplicate`, `builtin/ls-files.c`). `None` when `buf` is too large for `u32` offsets.
///
/// Proved for every input and every [`FindByte`] that meets its contract: `None` exactly when
/// `buf.len() > u32::MAX`; otherwise the spans denote exactly the records of `git.zsplit(buf)` (adjacent-deduped
/// with `dedup`), lie in `buf`, are in order with strictly increasing starts, and every record is NUL-free.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        match r {
            None => buf@.len() > u32::MAX,
            Some(spans) => buf@.len() <= u32::MAX && spans_ok(buf@, spans@) && spans_view(buf@, spans@) == (
            if dedup {
                dedup_adjacent(zsplit_spec(buf@))
            } else {
                zsplit_spec(buf@)
            }),
        },
))]
pub(crate) fn split_spans<F: FindByte>(
    find: &F,
    buf: &[u8],
    dedup: bool,
) -> Option<Vec<(u32, u32)>> {
    let n = buf.len();
    let Ok(_) = u32::try_from(n) else {
        return None;
    };
    let mut spans: Vec<(u32, u32)> = Vec::new();
    // `s.strip('\0')`: `buf[first..last]`.
    let mut first: usize = 0;
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(buf@.subrange(0, n as int) =~= buf@);
    }
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            first <= n == buf.len(),
            lstrip0(buf@) == lstrip0(buf@.subrange(first as int, n as int)),
        decreases n - first,
    ))]
    while first < n && buf.get(first) == Some(&0) {
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(buf@.subrange(first as int, n as int).drop_first() =~= buf@.subrange(
                first as int + 1,
                n as int,
            ));
        }
        first = first.saturating_add(1);
    }
    let mut last: usize = n;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            first <= last <= n == buf.len(),
            lstrip0(buf@) == buf@.subrange(first as int, n as int),
            rstrip0(lstrip0(buf@)) == rstrip0(buf@.subrange(first as int, last as int)),
        decreases last,
    ))]
    while last > first && buf.get(last.saturating_sub(1)) == Some(&0) {
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(buf@.subrange(first as int, last as int).drop_last() =~= buf@.subrange(
                first as int,
                last as int - 1,
            ));
        }
        last = last.saturating_sub(1);
    }
    if first == last {
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(spans_view(buf@, spans@) =~= Seq::<Seq<u8>>::empty());
        }
        return Some(spans);
    }
    #[cfg(verus_keep_ghost)]
    proof_decl! {
        assert(strip0(buf@) == buf@.subrange(first as int, last as int));
        lemma_split_win_shift(buf@, first as int, first as int, first as int, last as int);
        assert(zsplit_spec(buf@) == split_win(buf@, first as int, first as int, last as int));
        let ghost mut recs: Seq<Seq<u8>> = Seq::empty();
        assert(recs + split_win(buf@, first as int, first as int, last as int) =~= split_win(
            buf@,
            first as int,
            first as int,
            last as int,
        ));
    }
    // The start of the current record, and the last span pushed (valid once `spans` is non-empty).
    let mut s: usize = first;
    let mut prev: (usize, usize) = (0, 0);
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            first <= s < last <= n == buf.len(),
            n <= u32::MAX,
            buf@[last - 1] != 0u8,
            spans_view(buf@, spans@) == (if dedup {
                dedup_adjacent(recs)
            } else {
                recs
            }),
            spans@.len() > 0 ==> prev.0 == spans@.last().0 && prev.1 == spans@.last().1,
            forall|k: int, l: int|
                #![trigger spans@[k], spans@[l]]
                0 <= k < l < spans@.len() ==> spans@[k].1 < spans@[l].0 && spans@[k].0 < spans@[l].0,
            forall|k: int|
                0 <= k < spans@.len() ==> nul_free(
                    #[trigger] buf@.subrange(spans@[k].0 as int, spans@[k].1 as int),
                ),
        invariant_except_break
            split_win(buf@, first as int, first as int, last as int) == recs + split_win(
                buf@,
                s as int,
                s as int,
                last as int,
            ),
            forall|k: int| 0 <= k < spans@.len() ==> #[trigger] spans@[k].0 <= spans@[k].1 < s,
        ensures
            split_win(buf@, first as int, first as int, last as int) == recs,
            spans_view(buf@, spans@) == (if dedup {
                dedup_adjacent(recs)
            } else {
                recs
            }),
            spans_ok(buf@, spans@),
        decreases last - s,
    ))]
    while s < last {
        // The end of the current record: the next NUL before `last`, else `last`.
        let at = match find.find_byte(0, buf, s) {
            Some(i) if i < last => i,
            _ => last,
        };
        #[cfg(verus_keep_ghost)]
        proof_decl! {
            lemma_split_win_skip(buf@, s as int, s as int, at as int, last as int);
            let ghost rec = buf@.subrange(s as int, at as int);
            let ghost old_recs = recs;
            let ghost old_spans = spans@;
            assert(nul_free(rec)) by {
                assert forall|j: int| 0 <= j < rec.len() implies #[trigger] rec[j] != 0u8 by {
                    assert(rec[j] == buf@[s + j]);
                }
            }
        }
        let keep = !dedup || spans.is_empty() || !same(buf, prev.0, prev.1, s, at);
        if keep {
            let (Ok(s32), Ok(at32)) = (u32::try_from(s), u32::try_from(at)) else {
                return None;
            };
            spans.push((s32, at32));
            prev = (s, at);
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(spans_view(buf@, spans@) =~= spans_view(buf@, old_spans).push(rec));
                assert(buf@.subrange(spans@.last().0 as int, spans@.last().1 as int) == rec);
            }
        }
        #[cfg(verus_keep_ghost)]
        proof! {
            recs = recs.push(rec);
            assert(recs.drop_last() =~= old_recs);
        }
        if at >= last {
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(old_recs + split_win(buf@, s as int, s as int, last as int) =~= recs);
            }
            break;
        }
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(old_recs + split_win(buf@, s as int, s as int, last as int) =~= recs
                + split_win(buf@, at as int + 1, at as int + 1, last as int));
        }
        s = at.saturating_add(1);
    }
    Some(spans)
}
