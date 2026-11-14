//! Spec of kernel K5a, `git.zsplit` (docs/VERIFICATION.md §9.2, ADR-095), shared with K5b (`ZList::split`).
//!
//! Upstream: pre-commit 4.7.0 `pre_commit/git.py:19-24`:
//!
//! ```python
//! def zsplit(s: str) -> list[str]:
//!     s = s.strip('\0')
//!     if s:
//!         return s.split('\0')
//!     else:
//!         return []
//! ```
//!
//! `zsplit_spec` is that function on bytes (snp keeps git's output as bytes, D-CRASH-12). `split_nul` is Python's
//! `bytes.split(b'\0')`: always at least one field, interior empty fields kept. It is written as a left-to-right
//! scan (`split_win`) that closes a record at every NUL and the last record at the end, so the step lemmas below
//! match how the kernels walk the buffer. Properties (lemmas):
//!
//! - `lemma_split_nul_step`: the one-record step the unverified `ZSplit::next` glue takes (records up to the first
//!   NUL, then the split of the rest; no NUL: the whole rest is the last record).
//! - `lemma_split_win_nul_free` / `lemma_zsplit_nul_free`: every record is NUL-free.
//! - `lemma_split_win_join`: joining the records with NUL gives the input back (no byte lost or added).
//! - `lemma_strip0`: `strip0` removes exactly the leading and trailing NULs.
use vstd::prelude::*;

verus! {

/// No byte of `s` is NUL.
pub open spec fn nul_free(s: Seq<u8>) -> bool {
    forall|j: int| 0 <= j < s.len() ==> #[trigger] s[j] != 0u8
}

/// The contract of a byte search (`FindByte::find_byte`, `find_nul`): `Some(i)` is the first index at or after
/// `from` holding `needle`; `None` means no index at or after `from` holds it.
pub open spec fn first_from(h: Seq<u8>, needle: u8, from: int, r: Option<usize>) -> bool {
    match r {
        Some(i) => from <= i < h.len() && h[i as int] == needle && forall|j: int|
            from <= j < i ==> #[trigger] h[j] != needle,
        None => forall|j: int| from <= j < h.len() ==> #[trigger] h[j] != needle,
    }
}

/// `s.lstrip(b'\0')`.
pub open spec fn lstrip0(s: Seq<u8>) -> Seq<u8>
    decreases s.len(),
{
    if s.len() > 0 && s[0] == 0u8 {
        lstrip0(s.drop_first())
    } else {
        s
    }
}

/// `s.rstrip(b'\0')`.
pub open spec fn rstrip0(s: Seq<u8>) -> Seq<u8>
    decreases s.len(),
{
    if s.len() > 0 && s.last() == 0u8 {
        rstrip0(s.drop_last())
    } else {
        s
    }
}

/// `s.strip(b'\0')`.
pub open spec fn strip0(s: Seq<u8>) -> Seq<u8> {
    rstrip0(lstrip0(s))
}

/// The records of `b[st..hi]` split on NUL, for a scan at `j` whose current record started at `st`: a NUL at `j`
/// closes the record `b[st..j]`, the end closes the last one.
pub open spec fn split_win(b: Seq<u8>, st: int, j: int, hi: int) -> Seq<Seq<u8>>
    decreases hi - j,
{
    if j >= hi {
        seq![b.subrange(st, hi)]
    } else if b[j] == 0u8 {
        seq![b.subrange(st, j)] + split_win(b, j + 1, j + 1, hi)
    } else {
        split_win(b, st, j + 1, hi)
    }
}

/// Python `s.split(b'\0')`.
pub open spec fn split_nul(s: Seq<u8>) -> Seq<Seq<u8>> {
    split_win(s, 0, 0, s.len() as int)
}

/// `git.zsplit(s)` (`git.py:19-24`): `[]` when `s.strip(b'\0')` is empty, else its `split(b'\0')`.
pub open spec fn zsplit_spec(s: Seq<u8>) -> Seq<Seq<u8>> {
    let t = strip0(s);
    if t.len() == 0 {
        Seq::empty()
    } else {
        split_nul(t)
    }
}

/// `b'\0'.join(rs)` (for a non-empty `rs`).
pub open spec fn join0(rs: Seq<Seq<u8>>) -> Seq<u8>
    decreases rs.len(),
{
    if rs.len() <= 1 {
        if rs.len() == 0 {
            Seq::empty()
        } else {
            rs[0]
        }
    } else {
        rs[0] + seq![0u8] + join0(rs.drop_first())
    }
}

/// Skipping NUL-free bytes does not change the records.
pub proof fn lemma_split_win_skip(b: Seq<u8>, st: int, j: int, i: int, hi: int)
    requires
        j <= i <= hi,
        forall|k: int| j <= k < i ==> #[trigger] b[k] != 0u8,
    ensures
        split_win(b, st, j, hi) == split_win(b, st, i, hi),
    decreases i - j,
{
    if j < i {
        lemma_split_win_skip(b, st, j + 1, i, hi);
    }
}

/// A window of a sequence splits like the same window of the whole sequence.
pub proof fn lemma_split_win_shift(b: Seq<u8>, lo: int, st: int, j: int, hi: int)
    requires
        0 <= lo <= st <= j <= hi <= b.len(),
    ensures
        split_win(b.subrange(lo, hi), st - lo, j - lo, hi - lo) == split_win(b, st, j, hi),
    decreases hi - j,
{
    let w = b.subrange(lo, hi);
    if j >= hi {
        assert(w.subrange(st - lo, hi - lo) =~= b.subrange(st, hi));
    } else {
        assert(w[j - lo] == b[j]);
        if b[j] == 0u8 {
            assert(w.subrange(st - lo, j - lo) =~= b.subrange(st, j));
            lemma_split_win_shift(b, lo, j + 1, j + 1, hi);
        } else {
            lemma_split_win_shift(b, lo, st, j + 1, hi);
        }
    }
}

/// The one-record step of `ZSplit::next`: with `i` the first NUL of `r` (or `|r|` when there is none), the split
/// of `r` is `r[..i]` followed by the split of `r[i+1..]`, or just `[r]`.
pub proof fn lemma_split_nul_step(r: Seq<u8>, i: int)
    requires
        0 <= i <= r.len(),
        forall|k: int| 0 <= k < i ==> #[trigger] r[k] != 0u8,
        i == r.len() || r[i] == 0u8,
    ensures
        i == r.len() ==> split_nul(r) == seq![r],
        i < r.len() ==> split_nul(r) == seq![r.subrange(0, i)] + split_nul(
            r.subrange(i + 1, r.len() as int),
        ),
{
    let n = r.len() as int;
    lemma_split_win_skip(r, 0, 0, i, n);
    if i == n {
        assert(r.subrange(0, n) =~= r);
    } else {
        lemma_split_win_shift(r, i + 1, i + 1, i + 1, n);
    }
}

/// A split never has zero records.
pub proof fn lemma_split_win_len(b: Seq<u8>, st: int, j: int, hi: int)
    ensures
        split_win(b, st, j, hi).len() >= 1,
    decreases hi - j,
{
    if j < hi {
        if b[j] == 0u8 {
            lemma_split_win_len(b, j + 1, j + 1, hi);
        } else {
            lemma_split_win_len(b, st, j + 1, hi);
        }
    }
}

/// Every record of a split is NUL-free.
pub proof fn lemma_split_win_nul_free(b: Seq<u8>, st: int, j: int, hi: int)
    requires
        0 <= st <= j <= hi <= b.len(),
        forall|k: int| st <= k < j ==> #[trigger] b[k] != 0u8,
    ensures
        forall|r: int|
            0 <= r < split_win(b, st, j, hi).len() ==> nul_free(
                #[trigger] split_win(b, st, j, hi)[r],
            ),
    decreases hi - j,
{
    let s = split_win(b, st, j, hi);
    if j >= hi {
        assert forall|r: int| 0 <= r < s.len() implies nul_free(#[trigger] s[r]) by {
            assert forall|k: int| 0 <= k < s[r].len() implies #[trigger] s[r][k] != 0u8 by {
                assert(s[r][k] == b[st + k]);
            }
        }
    } else if b[j] == 0u8 {
        lemma_split_win_nul_free(b, j + 1, j + 1, hi);
        let rest = split_win(b, j + 1, j + 1, hi);
        assert(s == seq![b.subrange(st, j)] + rest);
        assert forall|r: int| 0 <= r < s.len() implies nul_free(#[trigger] s[r]) by {
            if r == 0 {
                assert forall|k: int| 0 <= k < s[r].len() implies #[trigger] s[r][k] != 0u8 by {
                    assert(s[r][k] == b[st + k]);
                }
            } else {
                assert(s[r] == rest[r - 1]);
            }
        }
    } else {
        lemma_split_win_nul_free(b, st, j + 1, hi);
        assert(s == split_win(b, st, j + 1, hi));
    }
}

/// Joining the records with NUL gives the window back.
pub proof fn lemma_split_win_join(b: Seq<u8>, st: int, j: int, hi: int)
    requires
        0 <= st <= j <= hi <= b.len(),
    ensures
        join0(split_win(b, st, j, hi)) == b.subrange(st, hi),
    decreases hi - j,
{
    if j < hi {
        if b[j] == 0u8 {
            lemma_split_win_join(b, j + 1, j + 1, hi);
            lemma_split_win_len(b, j + 1, j + 1, hi);
            let s = split_win(b, st, j, hi);
            assert(s.drop_first() =~= split_win(b, j + 1, j + 1, hi));
            assert(b.subrange(st, j) + seq![0u8] + b.subrange(j + 1, hi) =~= b.subrange(st, hi));
        } else {
            lemma_split_win_join(b, st, j + 1, hi);
        }
    }
}

/// `lstrip0(s)` is a suffix of `s` that is empty or starts with a non-NUL byte, after only NULs.
pub proof fn lemma_lstrip0(s: Seq<u8>)
    ensures
        lstrip0(s).len() <= s.len(),
        lstrip0(s) == s.subrange(s.len() - lstrip0(s).len(), s.len() as int),
        forall|k: int| 0 <= k < s.len() - lstrip0(s).len() ==> #[trigger] s[k] == 0u8,
        lstrip0(s).len() == 0 || lstrip0(s)[0] != 0u8,
    decreases s.len(),
{
    if s.len() > 0 && s[0] == 0u8 {
        lemma_lstrip0(s.drop_first());
        assert forall|k: int| 0 <= k < s.len() - lstrip0(s).len() implies #[trigger] s[k]
            == 0u8 by {
            if k > 0 {
                assert(s[k] == s.drop_first()[k - 1]);
            }
        }
        assert(s.drop_first().subrange(
            s.drop_first().len() - lstrip0(s).len(),
            s.drop_first().len() as int,
        ) =~= s.subrange(s.len() - lstrip0(s).len(), s.len() as int));
    } else {
        assert(s.subrange(0, s.len() as int) =~= s);
    }
}

/// `rstrip0(s)` is a prefix of `s` that is empty or ends with a non-NUL byte, before only NULs.
pub proof fn lemma_rstrip0(s: Seq<u8>)
    ensures
        rstrip0(s).len() <= s.len(),
        rstrip0(s) == s.subrange(0, rstrip0(s).len() as int),
        forall|k: int| rstrip0(s).len() <= k < s.len() ==> #[trigger] s[k] == 0u8,
        rstrip0(s).len() == 0 || rstrip0(s).last() != 0u8,
    decreases s.len(),
{
    if s.len() > 0 && s.last() == 0u8 {
        lemma_rstrip0(s.drop_last());
        assert forall|k: int| rstrip0(s).len() <= k < s.len() implies #[trigger] s[k] == 0u8 by {
            if k < s.len() - 1 {
                assert(s[k] == s.drop_last()[k]);
            }
        }
        assert(s.drop_last().subrange(0, rstrip0(s).len() as int) =~= s.subrange(
            0,
            rstrip0(s).len() as int,
        ));
    } else {
        assert(s.subrange(0, s.len() as int) =~= s);
    }
}

/// `strip0(s) = s[lo..hi]` where `s[..lo]` and `s[hi..]` are all NUL and `s[lo..hi]` is empty or has non-NUL
/// bytes at both ends: `strip0` removes exactly the leading and trailing NULs.
pub proof fn lemma_strip0(s: Seq<u8>)
    ensures
        exists|lo: int, hi: int|
            #![trigger s.subrange(lo, hi)]
            0 <= lo <= hi <= s.len() && strip0(s) == s.subrange(lo, hi) && (forall|k: int|
                0 <= k < lo ==> #[trigger] s[k] == 0u8) && (forall|k: int|
                hi <= k < s.len() ==> #[trigger] s[k] == 0u8) && (lo == hi || (s[lo] != 0u8 && s[hi
                - 1] != 0u8)),
{
    let l = lstrip0(s);
    lemma_lstrip0(s);
    lemma_rstrip0(l);
    let lo = s.len() - l.len();
    let hi = lo + rstrip0(l).len();
    assert(strip0(s) =~= s.subrange(lo, hi));
    assert forall|k: int| hi <= k < s.len() implies #[trigger] s[k] == 0u8 by {
        assert(s[k] == l[k - lo]);
    }
    if lo < hi {
        assert(s[lo] == l[0]);
        assert(s[hi - 1] == rstrip0(l).last());
    }
}

/// Every record of `zsplit_spec(s)` is NUL-free.
pub proof fn lemma_zsplit_nul_free(s: Seq<u8>)
    ensures
        forall|r: int| 0 <= r < zsplit_spec(s).len() ==> nul_free(#[trigger] zsplit_spec(s)[r]),
{
    let t = strip0(s);
    if t.len() > 0 {
        lemma_split_win_nul_free(t, 0, 0, t.len() as int);
    }
}

} // verus!
