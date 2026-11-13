//! Spec of kernel K3, end-of-file-fixer (docs/VERIFICATION.md §9.1, ADR-095).
//!
//! Upstream: pre-commit-hooks v6.0.0 `pre_commit_hooks/end_of_file_fixer.py:9-49` (`fix_file`). Python seeks to
//! the last byte: an empty file is left alone; a last byte other than `\r`/`\n` gets `b'\n'` appended. Otherwise
//! it walks back over the trailing run of `\r`/`\n` bytes; a file made only of them is truncated to nothing, and
//! else the run is cut after its first line ending (`\n`, `\r\n` or `\r`, tried in that order).
//!
//! `fix` is that function on contents (spec F of §9.1, the same definition as Lean `SnpModels.Eof.fix`, which
//! Lean proves equal to a line-by-line transliteration of `fix_file`: `pyFixFile_eq`). The `*_action` and
//! `bytes_result` functions state exactly what each exec function of the kernel returns for every input, so the
//! kernel's contracts are `result == spec(inputs@)`; the relational properties are lemmas below:
//!
//! - E0: the kernel's verification conditions (no panic, no overflow, termination).
//! - E1: `bytes_result(s)` is `None` iff `fix(s) == s`, otherwise `Some(fix(s))` (by definition, and
//!   `end_of_file_bytes` ensures it).
//! - E2: `lemma_run_action_apply`: the shipped path's action, applied to the file, gives `fix`.
//! - E3: `lemma_no_data_loss`.
//! - E4: `lemma_tail_locality`.
//! - E5: `lemma_fix_idempotent`, and `Some(v) ==> v != s` is part of `bytes_result`.
use crate::snp_hooks_pch_p0_kernels_eof::EofAction;
use vstd::prelude::*;

verus! {

/// `b'\n'` or `b'\r'`.
pub open spec fn eol(b: u8) -> bool {
    b == 10u8 || b == 13u8
}

/// `run(s)`: the length of the longest suffix of `s` made only of `\r`/`\n` bytes.
pub open spec fn run(s: Seq<u8>) -> nat
    decreases s.len(),
{
    if s.len() == 0 {
        0
    } else if eol(s.last()) {
        1 + run(s.drop_last())
    } else {
        0
    }
}

/// Spec F: `fix_file` as a function on the file's contents.
pub open spec fn fix(s: Seq<u8>) -> Seq<u8> {
    let n = s.len();
    let r = run(s);
    if n == 0 {
        s
    } else if r == 0 {
        s.push(10u8)
    } else if r == n {
        Seq::empty()
    } else {
        let p = n - r;
        if s[p] == 13u8 && r >= 2 && s[p + 1] == 10u8 {
            s.subrange(0, p + 2)
        } else {
            s.subrange(0, p + 1)
        }
    }
}

/// The new contents after applying `a` (`end_of_file_fixer::apply`: append `\n`, or truncate to `n` bytes).
pub open spec fn apply(s: Seq<u8>, a: EofAction) -> Seq<u8> {
    match a {
        EofAction::Keep => s,
        EofAction::Append => s.push(10u8),
        EofAction::Truncate(n) => s.subrange(0, n as int),
    }
}

/// `u64::saturating_add` in `int`.
pub open spec fn sat_u64(v: int) -> u64 {
    if v > u64::MAX {
        u64::MAX
    } else {
        v as u64
    }
}

/// Keep the run when it is exactly the kept line ending, else truncate after it.
pub open spec fn keep_or_truncate(r: int, position: int, keep: int) -> EofAction {
    if r == keep {
        EofAction::Keep
    } else {
        EofAction::Truncate(sat_u64(position + keep))
    }
}

/// What `end_of_file_run(head, r, len)` returns, for every input (consistent or not).
pub open spec fn run_action(head: Seq<u8>, r: int, len: int) -> EofAction {
    if len == 0 {
        EofAction::Keep
    } else if r == 0 {
        EofAction::Append
    } else if r >= len {
        EofAction::Truncate(0)
    } else if head.len() >= 1 && head[0] == 10u8 {
        keep_or_truncate(r, len - r, 1)
    } else if head.len() >= 1 && head[0] == 13u8 {
        if head.len() >= 2 && head[1] == 10u8 {
            keep_or_truncate(r, len - r, 2)
        } else {
            keep_or_truncate(r, len - r, 1)
        }
    } else {
        EofAction::Keep
    }
}

/// `min(run(tail), 2)`: how many bytes of the run `end_of_file` hands on as `head`.
pub open spec fn head_len(r: int) -> int {
    if r < 2 {
        r
    } else {
        2
    }
}

/// The first `min(run, 2)` bytes of the trailing run of `tail`.
pub open spec fn run_head(tail: Seq<u8>) -> Seq<u8> {
    let p = tail.len() - run(tail);
    tail.subrange(p, p + head_len(run(tail) as int))
}

/// What `end_of_file(tail, len)` returns, for every input.
pub open spec fn tail_action(tail: Seq<u8>, len: int) -> EofAction {
    if len == 0 || tail.len() == 0 {
        EofAction::Keep
    } else {
        run_action(run_head(tail), run(tail) as int, len)
    }
}

/// `end_of_file(s, |s|)`: the action computed from the whole file.
pub open spec fn file_action(s: Seq<u8>) -> EofAction {
    tail_action(s, s.len() as int)
}

/// What `end_of_file_bytes(s)` returns (E1, and `Some(v) ==> v != s` of E5).
pub open spec fn bytes_result(s: Seq<u8>) -> Option<Seq<u8>> {
    if fix(s) == s {
        None
    } else {
        Some(fix(s))
    }
}

/// The bytes of an optional `Vec`.
pub open spec fn opt_view(r: Option<Vec<u8>>) -> Option<Seq<u8>> {
    match r {
        Some(v) => Some(v@),
        None => None,
    }
}

/// `run(s)` is the maximal all-EOL suffix.
pub proof fn lemma_run_char(s: Seq<u8>)
    ensures
        run(s) <= s.len(),
        forall|i: int| s.len() - run(s) <= i < s.len() ==> eol(#[trigger] s[i]),
        run(s) < s.len() ==> !eol(s[s.len() - run(s) - 1]),
    decreases s.len(),
{
    if s.len() > 0 && eol(s.last()) {
        lemma_run_char(s.drop_last());
        assert forall|i: int| s.len() - run(s) <= i < s.len() implies eol(#[trigger] s[i]) by {
            if i < s.len() - 1 {
                assert(s.drop_last()[i] == s[i]);
            }
        }
        if run(s) < s.len() {
            assert(s.drop_last()[s.len() - run(s) - 1] == s[s.len() - run(s) - 1]);
        }
    }
}

/// An all-EOL suffix starting at `k` that cannot be extended to the left is the run.
pub proof fn lemma_run_unique(s: Seq<u8>, k: int)
    requires
        0 <= k <= s.len(),
        forall|i: int| k <= i < s.len() ==> eol(#[trigger] s[i]),
        k == 0 || !eol(s[k - 1]),
    ensures
        run(s) == s.len() - k,
{
    lemma_run_char(s);
    if run(s) < s.len() - k {
        assert(eol(s[s.len() - run(s) - 1]));
    } else if run(s) > s.len() - k {
        assert(eol(s[k - 1]));
    }
}

/// The run of `t ++ e`, where `t` ends in a non-EOL byte and `e` is all EOL, is `e`.
pub proof fn lemma_run_append(t: Seq<u8>, e: Seq<u8>)
    requires
        t.len() > 0,
        !eol(t.last()),
        forall|i: int| 0 <= i < e.len() ==> eol(#[trigger] e[i]),
    ensures
        run(t + e) == e.len(),
{
    let s = t + e;
    assert forall|i: int| t.len() <= i < s.len() implies eol(#[trigger] s[i]) by {
        assert(s[i] == e[i - t.len()]);
    }
    assert(s[t.len() - 1] == t.last());
    lemma_run_unique(s, t.len() as int);
}

/// E2 (the shipped path): for any file `s` of `len` bytes, `end_of_file_run` given the first `min(run(s), 2)`
/// bytes of the trailing run and `run(s)` returns the action whose application to `s` is `fix(s)`. The action
/// never truncates past the end, and its `Truncate` length is exact (no saturation).
pub proof fn lemma_run_action_apply(s: Seq<u8>, head: Seq<u8>, r: int)
    requires
        s.len() <= u64::MAX,
        r == run(s),
        head == s.subrange(s.len() - r, s.len() - r + head_len(r)),
    ensures
        apply(s, run_action(head, r, s.len() as int)) == fix(s),
        run_action(head, r, s.len() as int) matches EofAction::Truncate(n) ==> n < s.len(),
{
    lemma_run_char(s);
    let n = s.len() as int;
    if n == 0 || r == 0 || r >= n {
        if r >= n && n > 0 {
            assert(s.subrange(0, 0) =~= Seq::<u8>::empty());
        }
    } else {
        let p = n - r;
        assert(head[0] == s[p]);
        assert(eol(s[p]));
        if r >= 2 {
            assert(head[1] == s[p + 1]);
        }
        if r == 1 {
            assert(s.subrange(0, p + 1) =~= s);
        }
        if r == 2 {
            assert(s.subrange(0, p + 2) =~= s);
        }
    }
}

/// `file_action(s)` (`end_of_file` over the whole file) is the E2 action, so it applies to `fix(s)`.
pub proof fn lemma_tail_action_apply(s: Seq<u8>)
    requires
        s.len() <= u64::MAX,
    ensures
        apply(s, file_action(s)) == fix(s),
        file_action(s) matches EofAction::Truncate(n) ==> n < s.len(),
{
    lemma_run_char(s);
    lemma_run_action_apply(s, run_head(s), run(s) as int);
}

/// E3 (no data loss), for the whole-file action: a truncation only removes `\r`/`\n` bytes, and `\n` is
/// appended exactly when the file is non-empty and its last byte is not a line ending.
pub proof fn lemma_no_data_loss(s: Seq<u8>)
    requires
        s.len() <= u64::MAX,
    ensures
        file_action(s) matches EofAction::Truncate(n) ==> {
            &&& n < s.len()
            &&& forall|i: int| n <= i < s.len() ==> eol(#[trigger] s[i])
        },
        (file_action(s) is Append) <==> (s.len() > 0 && !eol(s.last())),
{
    lemma_run_char(s);
    lemma_tail_action_apply(s);
    if s.len() > 0 && !eol(s.last()) {
        assert(run(s) == 0);
    }
}

/// E4 (tail locality, the fuzz invariant of `pch_p0_end_of_file_fixer`): the action computed from a tail
/// `s[k..]` that still holds a non-EOL byte (or from the whole file) equals the whole-file action.
pub proof fn lemma_tail_locality(s: Seq<u8>, k: int)
    requires
        0 <= k <= s.len(),
        k == 0 || exists|i: int| k <= i < s.len() && !eol(#[trigger] s[i]),
    ensures
        tail_action(s.subrange(k, s.len() as int), s.len() as int) == file_action(s),
{
    if k > 0 {
        let t = s.subrange(k, s.len() as int);
        let j = choose|i: int| k <= i < s.len() && !eol(#[trigger] s[i]);
        lemma_run_char(s);
        lemma_run_char(t);
        // The run of `s` starts after `j`, so it lies inside `t`.
        assert(run(s) < s.len() - j) by {
            if run(s) >= s.len() - j {
                assert(eol(s[j]));
            }
        }
        assert forall|i: int| t.len() - run(s) <= i < t.len() implies eol(#[trigger] t[i]) by {
            assert(t[i] == s[i + k]);
        }
        assert(t[t.len() - run(s) - 1] == s[s.len() - run(s) - 1]);
        lemma_run_unique(t, t.len() - run(s));
        assert(run_head(t) =~= run_head(s));
    } else {
        assert(s.subrange(0, s.len() as int) =~= s);
    }
}

/// E5: the fix is idempotent.
pub proof fn lemma_fix_idempotent(s: Seq<u8>)
    ensures
        fix(fix(s)) == fix(s),
{
    let n = s.len();
    lemma_run_char(s);
    if n == 0 {
    } else if run(s) == 0 {
        let o = s.push(10u8);
        assert(o =~= s + seq![10u8]);
        assert(s.last() == s[n - 1]);
        lemma_run_append(s, seq![10u8]);
        assert(o.subrange(0, o.len() - 1 + 1) =~= o);
    } else if run(s) == n {
        assert(run(Seq::<u8>::empty()) == 0);
    } else {
        let p = n - run(s);
        let t = s.subrange(0, p);
        assert(t.last() == s[p - 1]);
        if s[p] == 13u8 && run(s) >= 2 && s[p + 1] == 10u8 {
            let o = s.subrange(0, p + 2);
            assert(o =~= t + seq![13u8, 10u8]);
            lemma_run_append(t, seq![13u8, 10u8]);
            assert(o.subrange(0, p + 2) =~= o);
        } else {
            let o = s.subrange(0, p + 1);
            assert(o =~= t + seq![s[p]]);
            lemma_run_append(t, seq![s[p]]);
            assert(o.subrange(0, p + 1) =~= o);
        }
    }
}

} // verus!
