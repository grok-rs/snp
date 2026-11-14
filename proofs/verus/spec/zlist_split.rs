//! Spec of kernel K5b, `ZList::split` (docs/VERIFICATION.md §9.2, ADR-095). The records themselves are K5a's
//! `zsplit_spec` (`proofs/verus/spec/zsplit.rs`, `git.py:19-24`).
//!
//! `ZList` keeps git's `-z` output as one buffer plus `(start, end)` offsets (`u32`, so the buffer is at most
//! `u32::MAX` bytes). `from_z` is `git.zsplit`; `from_z_dedup` is the fallback for git < 2.31, where
//! `ls-files --deduplicate` does not exist: it drops a record equal to the one before it (`builtin/ls-files.c`).
//! Properties (lemmas):
//!
//! - `lemma_dedup_adjacent_prev`: dropping a record equal to the last *kept* one (what the kernel does) is the same
//!   as dropping a record equal to the *previous* one (what git and `Vec::dedup` do).
use crate::spec::zsplit::nul_free;
use vstd::prelude::*;

verus! {

/// Adjacent dedup: a record equal to the last kept record is dropped.
pub open spec fn dedup_adjacent(s: Seq<Seq<u8>>) -> Seq<Seq<u8>>
    decreases s.len(),
{
    if s.len() == 0 {
        Seq::empty()
    } else {
        let r = dedup_adjacent(s.drop_last());
        if r.len() > 0 && r.last() == s.last() {
            r
        } else {
            r.push(s.last())
        }
    }
}

/// Adjacent dedup as git states it: a record equal to the previous record is dropped.
pub open spec fn dedup_prev(s: Seq<Seq<u8>>) -> Seq<Seq<u8>>
    decreases s.len(),
{
    if s.len() == 0 {
        Seq::empty()
    } else if s.len() >= 2 && s[s.len() - 2] == s.last() {
        dedup_prev(s.drop_last())
    } else {
        dedup_prev(s.drop_last()).push(s.last())
    }
}

/// The records a span list denotes in `b`.
pub open spec fn spans_view(b: Seq<u8>, spans: Seq<(u32, u32)>) -> Seq<Seq<u8>> {
    Seq::new(spans.len(), |k: int| b.subrange(spans[k].0 as int, spans[k].1 as int))
}

/// Spans lie in `b`, are in order (each ends before the next starts, so starts strictly increase) and denote
/// NUL-free records.
pub open spec fn spans_ok(b: Seq<u8>, spans: Seq<(u32, u32)>) -> bool {
    &&& forall|k: int| 0 <= k < spans.len() ==> #[trigger] spans[k].0 <= spans[k].1 <= b.len()
    &&& forall|k: int, l: int|
        #![trigger spans[k], spans[l]]
        0 <= k < l < spans.len() ==> spans[k].1 < spans[l].0 && spans[k].0 < spans[l].0
    &&& forall|k: int|
        0 <= k < spans.len() ==> nul_free(
            #[trigger] b.subrange(spans[k].0 as int, spans[k].1 as int),
        )
}

/// The last kept record is the last record.
pub proof fn lemma_dedup_adjacent_last(s: Seq<Seq<u8>>)
    requires
        s.len() > 0,
    ensures
        dedup_adjacent(s).len() > 0,
        dedup_adjacent(s).last() == s.last(),
{
}

/// Dropping a record equal to the last kept one is dropping a record equal to the previous one.
pub proof fn lemma_dedup_adjacent_prev(s: Seq<Seq<u8>>)
    ensures
        dedup_adjacent(s) == dedup_prev(s),
    decreases s.len(),
{
    if s.len() > 0 {
        lemma_dedup_adjacent_prev(s.drop_last());
        if s.len() >= 2 {
            lemma_dedup_adjacent_last(s.drop_last());
            assert(s.drop_last().last() == s[s.len() - 2]);
        }
    }
}

} // verus!
