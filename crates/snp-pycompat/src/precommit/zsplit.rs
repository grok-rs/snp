//! Verified kernel (ADR-095): the byte loops of `git.zsplit` (`git.py:19-24`), called by `super::zsplit` and
//! `super::ZSplit::next`.
//!
//! Spec and lemmas: `proofs/verus/spec/zsplit.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md §9.2 (K5a).
//! Plain Rust: the contract lives in `verus_keep_ghost` attributes and imports, which only Verus sets. The iterator
//! glue in `super` (slicing and the `ZSplit` state) is unverified; `lemma_split_nul_step` shows that its step, run
//! on these results, walks `split(b'\0')` record by record.
#[cfg(verus_keep_ghost)]
use crate::spec::zsplit::{first_from, lstrip0, rstrip0, strip0};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// `b.strip(b'\0')` as offsets (`git.py:20`): `Some((start, end))` when something is left, `b[start..end]` being
/// exactly the bytes between the leading and the trailing NULs; `None` when `b` is empty or all NUL, which is when
/// `git.zsplit` returns `[]` (`git.py:21-24`).
///
/// Proved for every input: `start < end <= b.len()` and `b[start..end] == strip0(b)`, or `None` exactly when
/// `strip0(b)` is empty.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        match r {
            Some((s, e)) => s < e <= b.len() && b@.subrange(s as int, e as int) == strip0(b@),
            None => strip0(b@).len() == 0,
        },
))]
pub(crate) fn strip_nul(b: &[u8]) -> Option<(usize, usize)> {
    let n = b.len();
    let mut start: usize = 0;
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(b@.subrange(0, n as int) =~= b@);
    }
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            start <= n == b.len(),
            lstrip0(b@) == lstrip0(b@.subrange(start as int, n as int)),
        decreases n - start,
    ))]
    while start < n && b.get(start) == Some(&0) {
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(b@.subrange(start as int, n as int).drop_first() =~= b@.subrange(
                start as int + 1,
                n as int,
            ));
        }
        start = start.saturating_add(1);
    }
    let mut end: usize = n;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            start <= end <= n == b.len(),
            lstrip0(b@) == b@.subrange(start as int, n as int),
            rstrip0(lstrip0(b@)) == rstrip0(b@.subrange(start as int, end as int)),
        decreases end,
    ))]
    while end > start && b.get(end.saturating_sub(1)) == Some(&0) {
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(b@.subrange(start as int, end as int).drop_last() =~= b@.subrange(
                start as int,
                end as int - 1,
            ));
        }
        end = end.saturating_sub(1);
    }
    if start == end {
        return None;
    }
    Some((start, end))
}

/// The first NUL of `b` (`bytes.split`'s field end, `git.py:22`), or `None` when `b` has none.
///
/// Proved for every input: `Some(i)` has `b[i] == 0` and no NUL before it; `None` means `b` has no NUL.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        first_from(b@, 0u8, 0, r),
))]
pub(crate) fn find_nul(b: &[u8]) -> Option<usize> {
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            i <= b.len(),
            forall|j: int| 0 <= j < i ==> b@[j] != 0u8,
        decreases b.len() - i,
    ))]
    while i < b.len() {
        if b.get(i) == Some(&0) {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}
