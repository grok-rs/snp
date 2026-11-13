//! Generic lemmas over sequences of integers (ADR-095, docs/VERIFICATION.md §2.4).
use vstd::prelude::*;

verus! {

/// `Σ s[lo..hi]` as a mathematical integer (never overflows); 0 for an empty or reversed range.
pub open spec fn sum_range(s: Seq<usize>, lo: int, hi: int) -> int
    decreases hi - lo,
{
    if hi <= lo {
        0
    } else {
        sum_range(s, lo, hi - 1) + s[hi - 1] as int
    }
}

/// A sum of `usize` values is non-negative.
pub proof fn lemma_sum_range_nonneg(s: Seq<usize>, lo: int, hi: int)
    ensures
        sum_range(s, lo, hi) >= 0,
    decreases hi - lo,
{
    if hi > lo {
        lemma_sum_range_nonneg(s, lo, hi - 1);
    }
}

/// Extending the range on the right never decreases the sum.
pub proof fn lemma_sum_range_mono(s: Seq<usize>, lo: int, hi1: int, hi2: int)
    requires
        lo <= hi1 <= hi2,
    ensures
        sum_range(s, lo, hi1) <= sum_range(s, lo, hi2),
    decreases hi2 - hi1,
{
    if hi1 < hi2 {
        lemma_sum_range_mono(s, lo, hi1, hi2 - 1);
    }
}

} // verus!
