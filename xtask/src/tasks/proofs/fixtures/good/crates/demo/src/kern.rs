//! Verified kernel (ADR-095): a demo kernel for the guard unit tests (not compiled).
use std::ops::Range;

#[cfg(verus_keep_ghost)]
use crate::spec::demo::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// A byte finder the kernel calls; production injects a fast one.
pub(crate) trait FindByte {
    /// The first `needle` at or after `from` in `hay`.
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        requires from <= hay.len(),
        ensures first_from(hay@, needle, from as int, r),
    ))]
    fn find_byte(&self, needle: u8, hay: &[u8], from: usize) -> Option<usize>;
}

/// The scalar reference finder.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive(Debug))]
pub(crate) struct Scalar;

impl FindByte for Scalar {
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures first_from(hay@, needle, from as int, r),
    ))]
    fn find_byte(&self, needle: u8, hay: &[u8], from: usize) -> Option<usize> {
        let mut i = from;
        #[cfg_attr(verus_keep_ghost, verus_spec(
            invariant from <= i,
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

/// The ranges of `hay` between `sep` bytes.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures spans_ok(r@, hay@),
))]
#[cfg_attr(verus_keep_ghost, verifier::rlimit(20))]
#[expect(clippy::arithmetic_side_effects, reason = "verus: i < hay.len() <= usize::MAX")]
pub(crate) fn spans<F: FindByte>(f: &F, sep: u8, hay: &[u8]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut start: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant start <= hay.len(),
        decreases hay.len() - start,
    ))]
    loop {
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_demo();
        }
        match f.find_byte(sep, hay, start) {
            Some(i) => {
                out.push(start..i);
                start = i + 1;
            }
            None => {
                out.push(start..hay.len());
                return out;
            }
        }
    }
}

/// A private helper with a precondition: allowed (every caller is verified).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    requires x < usize::MAX,
))]
fn helper(x: usize) -> usize {
    x.saturating_add(Scalar::ONE)
}

impl Scalar {
    const ONE: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_adds_one() {
        assert_eq!(helper(1), 2);
    }
}
