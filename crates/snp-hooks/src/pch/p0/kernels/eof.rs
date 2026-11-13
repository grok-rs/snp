//! Verified kernel (ADR-095): end-of-file-fixer's decision (`end_of_file_fixer.py:9-49`), re-exported by `super`.
//!
//! Spec and lemmas: `proofs/verus/spec/eof.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md §9.1 (K3).
//! Plain Rust: the contract lives in `verus_keep_ghost` attributes and imports, which only Verus sets. The shipped
//! native (`end_of_file_fixer::tail_action`) scans the file's tail itself and calls [`end_of_file_run`].
#[cfg(verus_keep_ghost)]
use crate::spec::eof::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// What end-of-file-fixer does to a file (`end_of_file_fixer.fix_file`, `end_of_file_fixer.py:9-49`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
pub enum EofAction {
    /// Nothing (empty file, or exactly one line ending at the end).
    Keep,
    /// Append `b'\n'` (the last byte is not `\r`/`\n`; a plain `\n` even in CRLF files).
    Append,
    /// Truncate to this length (extra line endings removed; a file of only `\r`/`\n` bytes becomes empty).
    Truncate(u64),
}

/// The action for a file of `len` bytes whose last `tail.len()` bytes are `tail`. `tail` must hold at least one
/// byte that is not `\r`/`\n`, or be the whole file.
///
/// Proved: the result depends on `tail` only through its trailing `\r`/`\n` run, so any such tail gives the
/// whole-file action (E4), and for the whole file the action rewrites it to `fix_file`'s result (E2) while
/// removing nothing but trailing line endings (E3).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == tail_action(tail@, len as int),
))]
pub fn end_of_file(tail: &[u8], len: u64) -> EofAction {
    if len == 0 || tail.is_empty() {
        return EofAction::Keep;
    }
    // `start`: where the trailing run of `\r`/`\n` bytes begins.
    let mut start = tail.len();
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            start <= tail.len(),
            forall|i: int| start <= i < tail.len() ==> eol(#[trigger] tail@[i]),
        ensures
            start == 0 || !eol(tail@[start - 1]),
        decreases start,
    ))]
    while start > 0 {
        let i = start.saturating_sub(1);
        match tail.get(i) {
            Some(&b) if is_eol(b) => start = i,
            _ => break,
        }
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        lemma_run_unique(tail@, start as int);
    }
    let run = tail.len().saturating_sub(start);
    let head = tail
        .get(start..start.saturating_add(2).min(tail.len()))
        .unwrap_or_default();
    end_of_file_run(head, u64::try_from(run).ok().unwrap_or(u64::MAX), len)
}

/// `\r` or `\n`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == eol(b),
))]
const fn is_eol(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

/// The action for a non-empty file of `len` bytes that ends in a run of `run` `\r`/`\n` bytes whose first
/// `min(run, 2)` bytes are `head` (all [`end_of_file`] needs, so a caller can scan the tail in constant memory).
///
/// Python keeps the first line ending of the run, trying `\n`, `\r\n` and `\r` in that order
/// (`end_of_file_fixer.py:39-47`), truncates a file of only line endings to nothing (`:26-31`) and appends `\n`
/// after a last byte that is not one (`:18-22`). Proved: given those inputs for any file, applying the result
/// yields `fix_file`'s bytes and never truncates past the end (E2).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == run_action(head@, run as int, len as int),
))]
pub fn end_of_file_run(head: &[u8], run: u64, len: u64) -> EofAction {
    if len == 0 {
        return EofAction::Keep;
    }
    if run == 0 {
        return EofAction::Append;
    }
    if run >= len {
        // `tell() == 1` after reading offset 0: the whole file is line endings.
        return EofAction::Truncate(0);
    }
    let position = len.saturating_sub(run);
    // The length of the line ending that is kept: `b'\n'`, `b'\r\n'` or `b'\r'`, the first that `head` starts with.
    let keep: u64 = match head.first() {
        Some(&b'\n') => 1,
        Some(&b'\r') => {
            if matches!(head.get(1), Some(&b'\n')) {
                2
            } else {
                1
            }
        }
        _ => return EofAction::Keep,
    };
    if run == keep {
        return EofAction::Keep;
    }
    EofAction::Truncate(position.saturating_add(keep))
}

/// end-of-file-fixer over a whole file's bytes: `Some(new bytes)` when it changes.
///
/// Proved: the result is `None` exactly when `fix_file` leaves the bytes unchanged, otherwise `Some` of its new
/// bytes, which differ from the input (E1, E5); `fix_file` is idempotent (E5).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        opt_view(r) == bytes_result(content@),
))]
pub fn end_of_file_bytes(content: &[u8]) -> Option<Vec<u8>> {
    let len = u64::try_from(content.len()).ok().unwrap_or(u64::MAX);
    #[cfg(verus_keep_ghost)]
    proof! {
        lemma_tail_action_apply(content@);
    }
    match end_of_file(content, len) {
        EofAction::Keep => None,
        EofAction::Append => {
            let mut v = Vec::new();
            v.extend_from_slice(content);
            v.push(b'\n');
            Some(v)
        }
        EofAction::Truncate(n) => {
            let kept = match usize::try_from(n) {
                Ok(n) => content.get(..n).unwrap_or(content),
                Err(_) => content,
            };
            let mut v = Vec::new();
            v.extend_from_slice(kept);
            Some(v)
        }
    }
}
