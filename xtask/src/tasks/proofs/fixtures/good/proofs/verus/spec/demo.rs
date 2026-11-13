//! Fixture spec of the demo kernel.
use std::ops::Range;
use vstd::prelude::*;

verus! {

/// The first `needle` at or after `from`.
pub open spec fn first_from(h: Seq<u8>, needle: u8, from: int, r: Option<usize>) -> bool {
    match r {
        Some(i) => from <= i < h.len() && h[i as int] == needle,
        None => forall|j: int| from <= j < h.len() ==> h[j] != needle,
    }
}

/// The spans are in bounds.
pub open spec fn spans_ok(r: Seq<Range<usize>>, h: Seq<u8>) -> bool {
    forall|k: int| 0 <= k < r.len() ==> #[trigger] r[k].end <= h.len()
}

/// A lemma the kernel calls.
pub proof fn lemma_demo() {
}

} // verus!
