//! Verified kernel (ADR-095): end-of-file-fixer's backward tail scan (`end_of_file_fixer.py:24-37`), used by
//! `super::super::end_of_file_fixer::tail_action`.
//!
//! Spec and lemmas: `proofs/verus/spec/eof_tail.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md §9.2 (K6).
//! The shell reads the file backwards in blocks ([`block_before`] says which) and folds each block into a
//! [`TailFold`] ([`fold_block`]) until a block holds a byte that is not `\r`/`\n`; the state then holds exactly what
//! the K3 kernel `end_of_file_run` needs: the length of the file's trailing `\r`/`\n` run and its first
//! `min(run, 2)` bytes. Constant memory: the state is three words, and nothing is allocated per block or byte.
#[cfg(verus_keep_ghost)]
use crate::spec::eof::{eol, lemma_run_unique, run};
#[cfg(verus_keep_ghost)]
use crate::spec::eof_tail::{block_spec, fold_head, fold_run, head_view};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// The trailing run of `\r`/`\n` bytes seen so far by the backward scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
pub(crate) struct TailFold {
    /// The run's length (saturating; it never saturates for a file of at most `u64::MAX` bytes).
    pub(crate) run: u64,
    /// The run's first bytes: `head[..min(head_len, 2)]`.
    pub(crate) head: [u8; 2],
    /// How many bytes of `head` are valid.
    pub(crate) head_len: usize,
}

impl TailFold {
    /// The state before any block: an empty run.
    pub(crate) const EMPTY: Self = Self {
        run: 0,
        head: [0, 0],
        head_len: 0,
    };

    /// The run's first `min(run, 2)` bytes.
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            r@ == head_view(*self),
    ))]
    pub(crate) fn head(&self) -> &[u8] {
        let n = if self.head_len < 2 { self.head_len } else { 2 };
        let all = self.head.as_slice();
        match all.get(0..n) {
            Some(h) => h,
            None => all,
        }
    }
}

/// The block that ends at offset `from`, for blocks of `size` bytes read backwards from the end:
/// `Some((start, n))` with `start = from - min(size, from)` and `n = from - start`; `None` once the scan reached the
/// start of the file (`from == 0`), and for `size == 0`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == block_spec(from as int, size as int),
))]
pub(crate) fn block_before(from: u64, size: u64) -> Option<(u64, usize)> {
    if from == 0 || size == 0 {
        return None;
    }
    let start = from.saturating_sub(size);
    match usize::try_from(from.saturating_sub(start)) {
        Ok(n) => Some((start, n)),
        Err(_) => None,
    }
}

/// Folds `block`, the bytes just before the ones already scanned, into `st`: the run grows by the block's trailing
/// `\r`/`\n` run, and the new run's first bytes are the first two of that run followed by the old head. `done` is
/// true exactly when the block holds a byte that is not `\r`/`\n`: the run is then complete and no earlier block
/// needs reading.
///
/// Proved: folding the blocks of any file `s` from its end, for every block size `B ≥ 1`, yields `run(s)` and the
/// run's first `min(run(s), 2)` bytes (K3's E2 inputs), and the scan reads no block before the one holding the last
/// byte that is not `\r`/`\n` (`lemma_scan` in the spec).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r.0.run == fold_run(st.run as int, block@),
        r.0.head_len <= 2,
        head_view(r.0) == fold_head(head_view(st), block@),
        r.1 == (run(block@) < block@.len()),
))]
pub(crate) fn fold_block(st: TailFold, block: &[u8]) -> (TailFold, bool) {
    // `start`: where the block's trailing run of `\r`/`\n` bytes begins.
    let mut start = block.len();
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            start <= block.len(),
            forall|i: int| start <= i < block.len() ==> eol(#[trigger] block@[i]),
        ensures
            start == 0 || !eol(block@[start - 1]),
        decreases start,
    ))]
    while start > 0 {
        let i = start.saturating_sub(1);
        match block.get(i) {
            Some(&b) if is_eol(b) => start = i,
            _ => break,
        }
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        lemma_run_unique(block@, start as int);
    }
    let new = block.len().saturating_sub(start);
    let run = st
        .run
        .saturating_add(u64::try_from(new).ok().unwrap_or(u64::MAX));
    let old_len = if st.head_len < 2 { st.head_len } else { 2 };
    // The bytes of this block precede the ones already seen: they become the run's first bytes.
    let (head, head_len) = match (block.get(start), block.get(start.saturating_add(1))) {
        (Some(&b0), Some(&b1)) => ([b0, b1], 2),
        (Some(&b0), None) => {
            if old_len >= 1 {
                ([b0, st.head[0]], 2)
            } else {
                ([b0, 0], 1)
            }
        }
        _ => (st.head, old_len),
    };
    (
        TailFold {
            run,
            head,
            head_len,
        },
        start > 0,
    )
}

/// `\r` or `\n`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == eol(b),
))]
const fn is_eol(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}
