//! Verified kernel (ADR-095): the core of `xargs.partition` (`xargs.py:76-117`), called by `super::partition`.
//!
//! Spec and lemmas: `proofs/verus/spec/xargs_partition.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md
//! §9.1 (K1). Plain Rust: the contract lives in `verus_keep_ghost` attributes and imports, which only Verus sets.
use std::ops::Range;

#[cfg(verus_keep_ghost)]
use crate::spec::lib::seq::{lemma_sum_range_nonneg, sum_range};
#[cfg(verus_keep_ghost)]
use crate::spec::xargs_partition::{in_budget, maximal, oversize, partition_post};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Greedy partition of `lens` under a command-length and an argument-count budget.
///
/// `lens[j]` is `cmdlen(files[j]) + 1`, `base` is `_command_length(*cmd) + 1`, `max_len` is `_max_length` and
/// `max_args` is `max(4, ceil(n / jobs))` (`xargs.py:76-117`). The result tiles `0..lens.len()` with contiguous,
/// in-order ranges (`[0..0]` for no input, otherwise none empty); each range takes at most `max_args` arguments,
/// keeps `base + Σ lens[range] <= max_len`, and every range but the last could not take one more argument.
/// `Err(i)` names the first argument that fits no command (Python's `ArgumentTooLongError`). Sums never wrap:
/// like Python's unbounded integers, an overflowing sum does not fit. Proved for every input (P0-P6), so the
/// result is the one Python returns.
#[cfg_attr(verus_keep_ghost, verus_spec(res =>
    ensures
        partition_post(res, lens@, base as int, max_len as int, max_args as int),
))]
pub(crate) fn partition_lens(
    lens: &[usize],
    base: usize,
    max_len: usize,
    max_args: usize,
) -> Result<Vec<Range<usize>>, usize> {
    let n = lens.len();
    let mut ret: Vec<Range<usize>> = Vec::new();
    let mut start: usize = 0;
    let mut count: usize = 0;
    let mut total: usize = base;
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            n == lens.len(),
            start <= i <= n,
            count == i - start,
            count <= max_args,
            total == base + sum_range(lens@, start as int, i as int),
            count > 0 ==> total <= max_len,
            count == 0 ==> ret@.len() == 0 || i < n,
            forall|j: int|
                0 <= j < i ==> !oversize(lens@, base as int, max_len as int, max_args as int, j),
            ret@.len() == 0 ==> start == 0,
            ret@.len() > 0 ==> ret@[0].start == 0 && ret@.last().end == start,
            forall|k: int| 0 <= k < ret@.len() ==> #[trigger] ret@[k].start < ret@[k].end,
            forall|k: int| 0 <= k < ret@.len() - 1 ==> #[trigger] ret@[k].end == ret@[k + 1].start,
            forall|k: int|
                0 <= k < ret@.len() ==> in_budget(
                    #[trigger] ret@[k], lens@, base as int, max_len as int, max_args as int),
            forall|k: int|
                0 <= k < ret@.len() ==> maximal(
                    #[trigger] ret@[k], lens@, base as int, max_len as int, max_args as int),
        decreases n - i, count,
    ))]
    while i < n {
        let Some(&arg_len) = lens.get(i) else {
            return Err(i);
        };
        // Python compares unbounded integers: an overflowing sum never fits.
        let fits = match total.checked_add(arg_len) {
            Some(t) => t <= max_len,
            None => false,
        };
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_sum_range_nonneg(lens@, start as int, i as int);
        }
        if fits && count < max_args {
            count = count.saturating_add(1);
            total = total.saturating_add(arg_len);
            i = i.saturating_add(1);
        } else if count == 0 {
            return Err(i);
        } else {
            // Exceeded the budget: emit a command and retry this argument in a fresh one.
            ret.push(start..i);
            start = i;
            count = 0;
            total = base;
        }
    }
    ret.push(start..i);
    Ok(ret)
}
