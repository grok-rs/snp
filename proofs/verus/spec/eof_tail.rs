//! Spec of kernel K6, end-of-file-fixer's backward tail scan (docs/VERIFICATION.md §9.2, ADR-095).
//!
//! Upstream: pre-commit-hooks v6.0.0 `pre_commit_hooks/end_of_file_fixer.py:24-37`: Python walks back from the last
//! byte one byte at a time while it reads `\r`/`\n`. snp reads the tail in blocks of `B` bytes instead
//! (`end_of_file_fixer.rs::tail_action`, `B = 4096`) and folds each block into the state `(run, head)` that K3's
//! `end_of_file_run` takes (`proofs/verus/spec/eof.rs`).
//!
//! `scan` is the shell's loop over the kernel: start from `(0, ε)` at the end of the file, ask [`block_spec`] for the
//! block before `from`, fold it ([`fold_spec`]), and stop after a block that holds a byte that is not `\r`/`\n` or
//! at the start of the file. The kernel's contracts are `result == spec(inputs@)` again; the properties are
//! lemmas:
//!
//! - T0: the kernel's verification conditions (no panic, no overflow, termination).
//! - T1 `lemma_scan`: for every file `s` (`|s| ≤ u64::MAX`) and every block size `1 ≤ B ≤ usize::MAX`, the scan ends
//!   with `(run(s), run_head(s))`: exactly the inputs of K3's E2 (`lemma_run_action_apply`), so the action the shell
//!   computes, applied to `s`, is `fix(s)`.
//! - T2 `lemma_scan`: reads stop at the first block (from the end) that holds a byte that is not `\r`/`\n`: the
//!   lowest offset read, `low`, satisfies `low ≤ |s| − run(s) − 1 < low + B`, and `low = 0` when the whole file is
//!   line endings; so at most `run(s) + B` bytes are read.
use crate::snp_hooks_pch_p0_kernels_eof_tail::TailFold;
use crate::spec::eof::{
    apply, eol, fix, head_len, lemma_run_action_apply, lemma_run_char, lemma_run_unique, run,
    run_action, run_head, sat_u64,
};
use vstd::prelude::*;

verus! {

/// The bytes a `TailFold` stands for: `head[..min(head_len, 2)]`.
pub(crate) open spec fn head_view(st: TailFold) -> Seq<u8> {
    st.head@.subrange(0, head_len(st.head_len as int))
}

/// The trailing `\r`/`\n` run of `b`.
pub open spec fn tail_run(b: Seq<u8>) -> Seq<u8> {
    b.subrange(b.len() - run(b), b.len() as int)
}

/// The first `min(|x|, 2)` bytes of `x`.
pub open spec fn take2(x: Seq<u8>) -> Seq<u8> {
    x.subrange(0, head_len(x.len() as int))
}

/// The run's length after folding `b` into a run of `r` bytes (`u64::saturating_add`, as the kernel computes it).
pub open spec fn fold_run(r: int, b: Seq<u8>) -> u64 {
    sat_u64(r + run(b))
}

/// The run's first bytes after folding `b` into a run whose first bytes are `h`.
pub open spec fn fold_head(h: Seq<u8>, b: Seq<u8>) -> Seq<u8> {
    take2(tail_run(b) + h)
}

/// `fold_block` on the abstract state `(run, head)`: the new state and `done`.
pub open spec fn fold_spec(st: (u64, Seq<u8>), b: Seq<u8>) -> ((u64, Seq<u8>), bool) {
    ((fold_run(st.0 as int, b), fold_head(st.1, b)), run(b) < b.len())
}

/// What `block_before(from, size)` returns, for every input.
pub open spec fn block_spec(from: int, size: int) -> Option<(u64, usize)> {
    if from <= 0 || size <= 0 {
        None
    } else {
        let start = if from > size {
            from - size
        } else {
            0
        };
        if from - start <= usize::MAX {
            Some((start as u64, (from - start) as usize))
        } else {
            None
        }
    }
}

/// The shell's loop (`tail_action`): the final state and the lowest offset read (`from` when nothing was read).
pub open spec fn scan(s: Seq<u8>, size: int, from: int, st: (u64, Seq<u8>)) -> ((u64, Seq<u8>), int)
    decreases from,
{
    match block_spec(from, size) {
        None => (st, from),
        Some((start, _n)) => {
            let (st2, done) = fold_spec(st, s.subrange(start as int, from));
            if done || start >= from || start < 0 {
                (st2, start as int)
            } else {
                scan(s, size, start as int, st2)
            }
        },
    }
}

/// The scan from `from` down, when everything after `from` is line endings and `st` describes it.
pub proof fn lemma_scan_from(s: Seq<u8>, size: int, from: int, st: (u64, Seq<u8>))
    requires
        1 <= size <= usize::MAX,
        0 <= from <= s.len() <= u64::MAX,
        forall|i: int| from <= i < s.len() ==> eol(#[trigger] s[i]),
        st.0 == s.len() - from,
        st.1 == s.subrange(from, from + head_len(s.len() - from)),
    ensures
        ({
            let (r, low) = scan(s, size, from, st);
            &&& r.0 == run(s)
            &&& r.1 == run_head(s)
            &&& 0 <= low <= from
            &&& run(s) == s.len() ==> low == 0
            &&& run(s) < s.len() ==> low <= s.len() - run(s) - 1 < low + size
        }),
    decreases from,
{
    let n = s.len() as int;
    if from == 0 {
        lemma_run_unique(s, 0);
        assert(run_head(s) == s.subrange(0, head_len(n)));
    } else {
        let start: int = if from > size {
            from - size
        } else {
            0
        };
        assert(block_spec(from, size) == Some::<(u64, usize)>(
            (start as u64, (from - start) as usize),
        ));
        let b = s.subrange(start, from);
        let rb = run(b) as int;
        lemma_run_char(b);
        let k = from - rb;
        assert forall|i: int| k <= i < n implies eol(#[trigger] s[i]) by {
            if i < from {
                assert(s[i] == b[i - start]);
            }
        }
        let (st2, done) = fold_spec(st, b);
        assert(tail_run(b) =~= s.subrange(k, from));
        assert(tail_run(b) + st.1 =~= s.subrange(k, from + head_len(n - from)));
        assert(st2.1 =~= s.subrange(k, k + head_len(n - k)));
        assert(st2.0 == n - k);
        if done {
            assert(s[k - 1] == b[b.len() - rb - 1]);
            lemma_run_unique(s, k);
            assert(scan(s, size, from, st) == (st2, start));
        } else {
            assert(k == start);
            assert(scan(s, size, from, st) == scan(s, size, start, st2));
            lemma_scan_from(s, size, start, st2);
        }
    }
}

/// T1 and T2: the scan of a whole file, for every block size `B ≥ 1`, yields K3's E2 inputs `(run(s),
/// run_head(s))`, so the action the shell hands on rewrites `s` to `fix(s)`, and it reads no block before the one
/// holding the last byte that is not a line ending.
pub proof fn lemma_scan(s: Seq<u8>, size: int)
    requires
        1 <= size <= usize::MAX,
        s.len() <= u64::MAX,
    ensures
        ({
            let (r, low) = scan(s, size, s.len() as int, (0u64, Seq::empty()));
            &&& r.0 == run(s)
            &&& r.1 == run_head(s)
            &&& apply(s, run_action(r.1, r.0 as int, s.len() as int)) == fix(s)
            &&& 0 <= low <= s.len()
            &&& run(s) == s.len() ==> low == 0
            &&& run(s) < s.len() ==> low <= s.len() - run(s) - 1 < low + size
        }),
{
    assert(s.subrange(s.len() as int, s.len() as int) =~= Seq::<u8>::empty());
    lemma_scan_from(s, size, s.len() as int, (0u64, Seq::empty()));
    lemma_run_char(s);
    lemma_run_action_apply(s, run_head(s), run(s) as int);
}

} // verus!
