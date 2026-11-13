//! Spec of kernel K1, the core of `xargs.partition` (docs/VERIFICATION.md §9.1, ADR-095).
//!
//! Upstream: pre-commit 4.7.0 `pre_commit/xargs.py:76-117` (`partition`). With `lens[j] = cmdlen(files[j]) + 1`,
//! `base = _command_length(*cmd) + 1`, `max_len = _max_length` and `max_args = max(4, ceil(n / jobs))`, Python
//! walks the arguments in order and appends `arg` to the current command while
//! `total_length + arg_length <= _max_length and len(ret_cmd) < max_args`; otherwise it raises
//! `ArgumentTooLongError(arg)` when the command is still empty, or emits the command and retries `arg` in a fresh
//! one. Python's integers are unbounded, so the spec's arithmetic is in `int`.
//!
//! Properties (P-numbers of §9.1): `partition_post` states P1-P5 for every input, the kernel's verification
//! condition adds P0 (no panic, no overflow, termination), and `lemma_partition_unique` is P6: at most one range
//! list satisfies P1-P4, so the kernel's result is the one Python's greedy loop produces.
use crate::spec::lib::seq::*;
use std::ops::Range;
use vstd::prelude::*;

verus! {

/// `over(j)`: argument `j` fits no command at all, not even a fresh one (Python raises `ArgumentTooLongError`).
pub open spec fn oversize(
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
    j: int,
) -> bool {
    max_args == 0 || base + lens[j] as int > max_len
}

/// P3: a non-empty range respects the argument-count and the command-length budget.
pub open spec fn in_budget(
    rg: Range<usize>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
) -> bool {
    rg.start < rg.end ==> {
        &&& rg.end - rg.start <= max_args
        &&& base + sum_range(lens, rg.start as int, rg.end as int) <= max_len
    }
}

/// P4: the range could not have taken the next argument: it is full, or that argument would overflow it.
pub open spec fn maximal(
    rg: Range<usize>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
) -> bool {
    &&& rg.end < lens.len()
    &&& (rg.end - rg.start == max_args || base + sum_range(lens, rg.start as int, rg.end as int)
        + lens[rg.end as int] as int > max_len)
}

/// P1-P4: `r` is a successful partition of `lens`. By `lemma_partition_unique` it determines `r` uniquely.
///
/// P1: contiguous, in-order ranges tiling `0..n` (nothing lost, duplicated or reordered). P2: zero inputs give
/// exactly `[0..0]`; otherwise no range is empty. P3: every range is within budget. P4: every range but the last
/// is maximal (greedy).
pub open spec fn partition_ok(
    r: Seq<Range<usize>>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
) -> bool {
    &&& r.len() >= 1
    &&& r[0].start == 0
    &&& r.last().end == lens.len()
    &&& forall|k: int| 0 <= k < r.len() ==> #[trigger] r[k].start <= r[k].end
    &&& forall|k: int| 0 <= k < r.len() - 1 ==> #[trigger] r[k].end == r[k + 1].start
    &&& lens.len() == 0 ==> r.len() == 1
    &&& lens.len() > 0 ==> forall|k: int| 0 <= k < r.len() ==> #[trigger] r[k].start < r[k].end
    &&& forall|k: int|
        0 <= k < r.len() ==> in_budget(#[trigger] r[k], lens, base, max_len, max_args)
    &&& forall|k: int|
        0 <= k < r.len() - 1 ==> maximal(#[trigger] r[k], lens, base, max_len, max_args)
}

/// The contract of `partition_lens`: P1-P4 on `Ok`, and P5 (`Ok` iff no argument is oversize; `Err(i)` names the
/// first oversize argument).
pub open spec fn partition_post(
    res: Result<Vec<Range<usize>>, usize>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
) -> bool {
    match res {
        Ok(r) => {
            &&& partition_ok(r@, lens, base, max_len, max_args)
            &&& forall|j: int| 0 <= j < lens.len() ==> !oversize(lens, base, max_len, max_args, j)
        },
        Err(i) => {
            &&& i < lens.len()
            &&& oversize(lens, base, max_len, max_args, i as int)
            &&& forall|j: int| 0 <= j < i ==> !oversize(lens, base, max_len, max_args, j)
        },
    }
}

/// Two ranges from the same start that are both in budget, and each either final (`end == n`) or maximal, end at
/// the same index.
pub proof fn lemma_end_determined(
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
    s: usize,
    e1: usize,
    e2: usize,
    last1: bool,
    last2: bool,
)
    requires
        s < e1 <= lens.len(),
        s < e2 <= lens.len(),
        in_budget((s..e1), lens, base, max_len, max_args),
        in_budget((s..e2), lens, base, max_len, max_args),
        last1 ==> e1 == lens.len(),
        !last1 ==> maximal((s..e1), lens, base, max_len, max_args),
        last2 ==> e2 == lens.len(),
        !last2 ==> maximal((s..e2), lens, base, max_len, max_args),
    ensures
        e1 == e2,
{
    if e1 < e2 {
        lemma_sum_range_mono(lens, s as int, e1 as int + 1, e2 as int);
    } else if e2 < e1 {
        lemma_sum_range_mono(lens, s as int, e2 as int + 1, e1 as int);
    }
}

/// In a tiling that ends at `n`, every range ends at or before `n`.
pub proof fn lemma_ends_bounded(r: Seq<Range<usize>>, n: int, k: int)
    requires
        r.len() >= 1,
        r.last().end == n,
        forall|j: int| 0 <= j < r.len() ==> #[trigger] r[j].start <= r[j].end,
        forall|j: int| 0 <= j < r.len() - 1 ==> #[trigger] r[j].end == r[j + 1].start,
        0 <= k < r.len(),
    ensures
        r[k].end <= n,
    decreases r.len() - k,
{
    if k < r.len() - 1 {
        lemma_ends_bounded(r, n, k + 1);
        assert(r[k].end == r[k + 1].start);
        assert(r[k + 1].start <= r[k + 1].end);
    }
}

/// Two successful partitions of a non-empty input agree on every common index.
proof fn lemma_prefix_eq(
    r1: Seq<Range<usize>>,
    r2: Seq<Range<usize>>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
    k: int,
)
    requires
        lens.len() > 0,
        partition_ok(r1, lens, base, max_len, max_args),
        partition_ok(r2, lens, base, max_len, max_args),
        0 <= k < r1.len(),
        k < r2.len(),
    ensures
        r1[k] == r2[k],
    decreases k,
{
    if k > 0 {
        lemma_prefix_eq(r1, r2, lens, base, max_len, max_args, k - 1);
        assert(r1[k - 1].end == r1[k].start);
        assert(r2[k - 1].end == r2[k].start);
    }
    lemma_ends_bounded(r1, lens.len() as int, k);
    lemma_ends_bounded(r2, lens.len() as int, k);
    assert(r1[k].start < r1[k].end);
    assert(r2[k].start < r2[k].end);
    assert(in_budget(r1[k], lens, base, max_len, max_args));
    assert(in_budget(r2[k], lens, base, max_len, max_args));
    let last1 = k == r1.len() - 1;
    let last2 = k == r2.len() - 1;
    if !last1 {
        assert(maximal(r1[k], lens, base, max_len, max_args));
    }
    if !last2 {
        assert(maximal(r2[k], lens, base, max_len, max_args));
    }
    lemma_end_determined(
        lens,
        base,
        max_len,
        max_args,
        r1[k].start,
        r1[k].end,
        r2[k].end,
        last1,
        last2,
    );
}

/// P6: `partition_ok` is a complete spec. At most one range list satisfies P1-P4, so `partition_lens` returns
/// exactly the partition Python's greedy loop builds (which satisfies P1-P4 by construction), and the proptest
/// `partition_matches_the_python_model` holds for the kernel on every input.
pub proof fn lemma_partition_unique(
    r1: Seq<Range<usize>>,
    r2: Seq<Range<usize>>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
)
    requires
        partition_ok(r1, lens, base, max_len, max_args),
        partition_ok(r2, lens, base, max_len, max_args),
    ensures
        r1 == r2,
{
    if lens.len() == 0 {
        assert(r1 =~= r2);
    } else {
        if r1.len() != r2.len() {
            let (a, b) = if r1.len() < r2.len() {
                (r1, r2)
            } else {
                (r2, r1)
            };
            let k = a.len() - 1;
            lemma_prefix_eq(r1, r2, lens, base, max_len, max_args, k);
            assert(a[k] == b[k]);
            assert(b[k].end == b[k + 1].start);
            assert(b[k + 1].start < b[k + 1].end);
            lemma_ends_bounded(b, lens.len() as int, k + 1);
        }
        assert forall|k: int| 0 <= k < r1.len() implies r1[k] == r2[k] by {
            lemma_prefix_eq(r1, r2, lens, base, max_len, max_args, k);
        }
        assert(r1 =~= r2);
    }
}

/// P6 for results: two results that both satisfy the contract are equal (the `Err` index is the first oversize
/// argument, and the `Ok` list is unique).
pub proof fn lemma_partition_post_unique(
    res1: Result<Vec<Range<usize>>, usize>,
    res2: Result<Vec<Range<usize>>, usize>,
    lens: Seq<usize>,
    base: int,
    max_len: int,
    max_args: int,
)
    requires
        partition_post(res1, lens, base, max_len, max_args),
        partition_post(res2, lens, base, max_len, max_args),
    ensures
        match (res1, res2) {
            (Ok(a), Ok(b)) => a@ == b@,
            (Err(i), Err(j)) => i == j,
            _ => false,
        },
{
    match (res1, res2) {
        (Ok(a), Ok(b)) => lemma_partition_unique(a@, b@, lens, base, max_len, max_args),
        (Err(i), Err(j)) => {
            if i < j {
                assert(!oversize(lens, base, max_len, max_args, i as int));
            } else if j < i {
                assert(!oversize(lens, base, max_len, max_args, j as int));
            }
        },
        (Ok(_), Err(i)) => {
            assert(!oversize(lens, base, max_len, max_args, i as int));
        },
        (Err(i), Ok(_)) => {
            assert(!oversize(lens, base, max_len, max_args, i as int));
        },
    }
}

} // verus!
