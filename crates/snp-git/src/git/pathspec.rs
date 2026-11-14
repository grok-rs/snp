//! Verified kernel (ADR-095): the D-DATA-19 pathspec lists of the stash checkouts, called by `super`.
//!
//! Which files the stash and its conflict rollback rewrite (ADR-093) is decided here: the paths of `git apply
//! --numstat -z` ([`numstat_spans`]), the `:(literal)` pathspecs fed to `git checkout` ([`literal_list`],
//! [`literal_spec`], [`literal_chunks`]), the sink's refusal of a path that would select more than itself
//! ([`checkout_refusal`]) and the rollback list ([`rollback_list`], over the [`SeenSet`] kernel trait).
//!
//! Spec and lemmas: `proofs/verus/spec/pathspec.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md §9.2
//! (K7). Plain Rust: the contract lives in `verus_keep_ghost` attributes and imports, which only Verus sets.

#[cfg(verus_keep_ghost)]
use crate::spec::pathspec::{
    chunks_ok, dedup, drop_empty, fresh, ints, lemma_nul_at, lemma_prepend_cat, lemma_tab_in, lit,
    lit_list, lit_sum, magic, nul_at, nul_free, numstat_post, ok_path, parse_from, prepend,
    rollback_spec, tab_in, views,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// The first NUL at or after `from` (`out.len()` when there is none).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    requires
        from <= out.len(),
    ensures
        r == nul_at(out@, from as int),
        from <= r <= out.len(),
))]
fn find_nul(out: &[u8], from: usize) -> usize {
    let mut j = from;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            from <= j <= out.len(),
            nul_at(out@, from as int) == nul_at(out@, j as int),
        decreases out.len() - j,
    ))]
    while j < out.len() {
        if out.get(j) == Some(&0) {
            return j;
        }
        j = j.saturating_add(1);
    }
    j
}

/// The first TAB in `out[from..end]` (`end` when there is none).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    requires
        from <= end <= out.len(),
    ensures
        r == tab_in(out@, from as int, end as int),
        from <= r <= end,
))]
fn find_tab(out: &[u8], from: usize, end: usize) -> usize {
    let mut j = from;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            from <= j <= end <= out.len(),
            tab_in(out@, from as int, end as int) == tab_in(out@, j as int, end as int),
        decreases end - j,
    ))]
    while j < end {
        if out.get(j) == Some(&b'\t') {
            return j;
        }
        j = j.saturating_add(1);
    }
    j
}

/// The paths of `git apply --numstat -z` output as byte spans of `out`, in order; `Err` is the span of the first
/// malformed record.
///
/// Records are `<added>\t<deleted>\t<path>\0` (`-\t-\t` for a binary file; git's `apply.c`, `numstat_patch_list`);
/// the path is everything after the second TAB, TABs and newlines included. Empty records are skipped. A rename or
/// copy record (`<added>\t<deleted>\t\0<old>\0<new>\0`, never produced for a stash patch) contributes both names,
/// which must be non-empty. A record without two TABs is an error. Proved for every input: the result is exactly
/// the spec `parse_from(out, 0)`, so every returned path is a non-empty, NUL-free slice of `out` (N2), and the
/// output git prints for any list of names parses back to exactly those names (N1).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        numstat_post(out@, r),
))]
pub(crate) fn numstat_spans(out: &[u8]) -> Result<Vec<(usize, usize)>, (usize, usize)> {
    let n = out.len();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            n == out.len(),
            i <= n,
            parse_from(out@, 0) == prepend(ints(spans@), parse_from(out@, i as int)),
        decreases n - i,
    ))]
    while i <= n {
        let k = find_nul(out, i);
        if k == i {
            // An empty record: skipped (the last one ends the output).
            if k >= n {
                #[cfg(verus_keep_ghost)]
                proof! {
                    assert(ints(spans@) + Seq::<(int, int)>::empty() =~= ints(spans@));
                }
                return Ok(spans);
            }
            i = k.saturating_add(1);
            continue;
        }
        let t1 = find_tab(out, i, k);
        let t2 = if t1 < k {
            find_tab(out, t1.saturating_add(1), k)
        } else {
            k
        };
        if t2 >= k {
            return Err((i, k));
        }
        let start = t2.saturating_add(1);
        if start < k {
            #[cfg(verus_keep_ghost)]
            proof_decl! {
                let ghost old_spans = spans@;
            }
            spans.push((start, k));
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(ints(spans@) =~= ints(old_spans) + seq![(start as int, k as int)]);
                lemma_prepend_cat(
                    ints(old_spans),
                    seq![(start as int, k as int)],
                    if k >= n { Ok(Seq::empty()) } else { parse_from(out@, k + 1) },
                );
            }
            if k >= n {
                #[cfg(verus_keep_ghost)]
                proof! {
                    assert(ints(spans@) + Seq::<(int, int)>::empty() =~= ints(spans@));
                }
                return Ok(spans);
            }
            i = k.saturating_add(1);
            continue;
        }
        // An empty path: a rename or copy record, whose two names are the next two records.
        if k >= n {
            return Err((i, k));
        }
        let k2 = find_nul(out, k.saturating_add(1));
        if k2 == k.saturating_add(1) || k2 >= n {
            return Err((i, k));
        }
        let k3 = find_nul(out, k2.saturating_add(1));
        if k3 == k2.saturating_add(1) {
            return Err((i, k));
        }
        #[cfg(verus_keep_ghost)]
        proof_decl! {
            let ghost old_spans = spans@;
        }
        spans.push((k.saturating_add(1), k2));
        spans.push((k2.saturating_add(1), k3));
        #[cfg(verus_keep_ghost)]
        proof! {
            let two = seq![(k + 1, k2 as int), (k2 + 1, k3 as int)];
            assert(ints(spans@) =~= ints(old_spans) + two);
            lemma_prepend_cat(
                ints(old_spans),
                two,
                if k3 >= n { Ok(Seq::empty()) } else { parse_from(out@, k3 + 1) },
            );
        }
        if k3 >= n {
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(ints(spans@) + Seq::<(int, int)>::empty() =~= ints(spans@));
            }
            return Ok(spans);
        }
        i = k3.saturating_add(1);
    }
    Ok(spans)
}

/// Appends `:(literal)`.
#[cfg_attr(verus_keep_ghost, verus_spec(
    ensures
        final(out)@ == old(out)@ + magic(),
))]
fn push_magic(out: &mut Vec<u8>) {
    out.push(b':');
    out.push(b'(');
    out.push(b'l');
    out.push(b'i');
    out.push(b't');
    out.push(b'e');
    out.push(b'r');
    out.push(b'a');
    out.push(b'l');
    out.push(b')');
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(out@ =~= old(out)@ + magic());
    }
}

/// `:(literal)<p>`: a pathspec that matches exactly the path `p`, without glob or magic.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r@ == lit(p@),
))]
pub(crate) fn literal_spec(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    push_magic(&mut out);
    out.extend_from_slice(p);
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(out@ =~= lit(p@));
    }
    out
}

/// `:(literal)<path>\0` per path, in order: the `--pathspec-file-nul` list of the checkout. Proved: for NUL-free
/// paths git reads back exactly one `:(literal)<path>` item per path, and the encoding is injective (N3).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r@ == lit_list(views(paths@)),
))]
#[expect(
    clippy::indexing_slicing,
    reason = "verus: every index is proved in bounds"
)]
pub(crate) fn literal_list(paths: &[&[u8]]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut k: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            k <= paths.len(),
            out@ == lit_list(views(paths@).take(k as int)),
        decreases paths.len() - k,
    ))]
    while k < paths.len() {
        let p = paths[k];
        #[cfg(verus_keep_ghost)]
        proof_decl! {
            let ghost before = out@;
        }
        push_magic(&mut out);
        out.extend_from_slice(p);
        out.push(0);
        #[cfg(verus_keep_ghost)]
        proof! {
            let l = views(paths@).take(k + 1);
            assert(l.drop_last() =~= views(paths@).take(k as int));
            assert(l.last() == p@);
            assert(out@ =~= before + lit(p@) + seq![0u8]);
        }
        k = k.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(views(paths@).take(paths.len() as int) =~= views(paths@));
    }
    out
}

/// `p` holds a NUL byte.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == !nul_free(p@),
))]
fn has_nul(p: &[u8]) -> bool {
    let mut j: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            j <= p.len(),
            forall|m: int| 0 <= m < j ==> p@[m] != 0u8,
        decreases p.len() - j,
    ))]
    while j < p.len() {
        if p.get(j) == Some(&0) {
            return true;
        }
        j = j.saturating_add(1);
    }
    false
}

/// The first path the checkout sink must refuse: an empty one (`:(literal)` with no name is the whole tree, the
/// checkout D-DATA-19 removed) or one holding a NUL (it would split into two items). `None`: every path names
/// exactly one file. Proved (N4).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        match r {
            None => forall|k: int| 0 <= k < paths@.len() ==> ok_path(#[trigger] paths@[k]@),
            Some(k) => {
                &&& k < paths@.len()
                &&& !ok_path(paths@[k as int]@)
                &&& forall|j: int| 0 <= j < k ==> ok_path(#[trigger] paths@[j]@)
            },
        },
))]
#[expect(
    clippy::indexing_slicing,
    reason = "verus: every index is proved in bounds"
)]
pub(crate) fn checkout_refusal(paths: &[&[u8]]) -> Option<usize> {
    let mut k: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            k <= paths.len(),
            forall|j: int| 0 <= j < k ==> ok_path(#[trigger] paths@[j]@),
        decreases paths.len() - k,
    ))]
    while k < paths.len() {
        let p = paths[k];
        if p.is_empty() || has_nul(p) {
            return Some(k);
        }
        k = k.saturating_add(1);
    }
    None
}

/// The argument chunks of the git < 2.25 checkout (`checkout -- :(literal)<path>…`): half-open index ranges of
/// `paths`, in order, none for an empty list.
///
/// A path joins the current chunk while the chunk's pathspecs, one separator byte after each, plus the new
/// pathspec stay within `budget` bytes; otherwise it starts the next chunk (so a pathspec longer than `budget`
/// gets a chunk of its own). Proved: the chunks tile the list, so concatenated they are the whole list; each
/// chunk of more than one path is within `budget`; each chunk but the last is maximal.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        chunks_ok(r@, views(paths@), budget as int),
))]
#[expect(
    clippy::indexing_slicing,
    reason = "verus: every index is proved in bounds"
)]
pub(crate) fn literal_chunks(paths: &[&[u8]], budget: usize) -> Vec<(usize, usize)> {
    let count = paths.len();
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    if count == 0 {
        return chunks;
    }
    let mut start: usize = 0;
    // The chunk's bytes so far (saturating; a chunk of two or more paths is within the budget, so never saturated).
    let mut bytes: usize = 0;
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            count == paths.len(),
            count > 0,
            start <= i <= count,
            i > 0 ==> start < i,
            i == 0 ==> start == 0 && chunks@.len() == 0,
            bytes as int == if lit_sum(views(paths@), start as int, i as int) > usize::MAX {
                usize::MAX as int
            } else {
                lit_sum(views(paths@), start as int, i as int)
            },
            i - start >= 2 ==> lit_sum(views(paths@), start as int, i as int) <= budget + 1,
            chunks@.len() == 0 ==> start == 0,
            chunks@.len() > 0 ==> chunks@[0].0 == 0 && chunks@.last().1 == start,
            forall|k: int| 0 <= k < chunks@.len() ==> (#[trigger] chunks@[k]).0 < chunks@[k].1,
            forall|k: int| 0 <= k < chunks@.len() - 1 ==> (#[trigger] chunks@[k]).1 == chunks@[k + 1].0,
            forall|k: int|
                0 <= k < chunks@.len() ==> (#[trigger] chunks@[k]).1 - chunks@[k].0 == 1
                    || lit_sum(views(paths@), chunks@[k].0 as int, chunks@[k].1 as int) <= budget + 1,
            forall|k: int|
                0 <= k < chunks@.len() ==> lit_sum(
                    views(paths@),
                    (#[trigger] chunks@[k]).0 as int,
                    chunks@[k].1 + 1,
                ) > budget + 1,
        decreases count - i,
    ))]
    while i < count {
        let path = paths[i];
        let width = path.len();
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(views(paths@)[i as int] == path@);
            assert(lit_sum(views(paths@), start as int, i + 1) == lit_sum(
                views(paths@),
                start as int,
                i as int,
            ) + width + 11);
        }
        if i > start {
            // The pathspec is `:(literal)` (10 bytes) plus the path; overflowing arithmetic never fits.
            let over = match bytes.checked_add(width) {
                Some(t) => match t.checked_add(10) {
                    Some(u) => u > budget,
                    None => true,
                },
                None => true,
            };
            if over {
                #[cfg(verus_keep_ghost)]
                proof_decl! {
                    let ghost old_chunks = chunks@;
                }
                chunks.push((start, i));
                #[cfg(verus_keep_ghost)]
                proof! {
                    assert(chunks@.last() == (start, i));
                    assert forall|k: int| 0 <= k < old_chunks.len() implies #[trigger] chunks@[k] == old_chunks[k] by {}
                }
                start = i;
                bytes = 0;
            }
        }
        bytes = bytes.saturating_add(width).saturating_add(11);
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(lit_sum(views(paths@), start as int, i + 1) == lit_sum(
                views(paths@),
                start as int,
                i as int,
            ) + width + 11);
        }
        i = i.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof_decl! {
        let ghost old_chunks = chunks@;
    }
    chunks.push((start, count));
    #[cfg(verus_keep_ghost)]
    proof! {
        assert forall|k: int| 0 <= k < old_chunks.len() implies #[trigger] chunks@[k] == old_chunks[k] by {}
    }
    chunks
}

/// First-seen membership over byte records: the kernel trait behind the rollback list's dedup (ADR-095 §2.6).
///
/// The production implementation (a hash set, `super::HashSeen`) is a trusted assumption, registered in
/// `proofs/registry.toml` with a differential test against the verified [`Scan`].
pub(crate) trait SeenSet {
    /// `r[i]`: `records[i]` does not occur before index `i`.
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            r@.len() == records@.len(),
            forall|i: int| 0 <= i < records@.len() ==> #[trigger] r@[i] == fresh(views(records@), i),
    ))]
    fn first_seen(&self, records: &[&[u8]]) -> Vec<bool>;
}

/// `a == b`, byte by byte.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == (a@ == b@),
))]
fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut j: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            a.len() == b.len(),
            j <= a.len(),
            forall|m: int| 0 <= m < j ==> a@[m] == b@[m],
        decreases a.len() - j,
    ))]
    while j < a.len() {
        if a.get(j) != b.get(j) {
            return false;
        }
        j = j.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(a@ =~= b@);
    }
    true
}

/// The verified reference [`SeenSet`]: compares each record with every earlier one (quadratic; the shell uses it
/// for short lists, where it beats hashing).
#[derive(Debug, Clone, Copy)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive(Debug))]
pub(crate) struct Scan;

impl SeenSet for Scan {
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            r@.len() == records@.len(),
            forall|i: int| 0 <= i < records@.len() ==> #[trigger] r@[i] == fresh(views(records@), i),
    ))]
    #[expect(
        clippy::indexing_slicing,
        reason = "verus: every index is proved in bounds"
    )]
    fn first_seen(&self, records: &[&[u8]]) -> Vec<bool> {
        let count = records.len();
        let mut mask: Vec<bool> = Vec::new();
        let mut i: usize = 0;
        #[cfg_attr(verus_keep_ghost, verus_spec(
            invariant
                count == records.len(),
                i <= count,
                mask@.len() == i,
                forall|k: int| 0 <= k < i ==> #[trigger] mask@[k] == fresh(views(records@), k),
            decreases count - i,
        ))]
        while i < count {
            let rec = records[i];
            let mut seen_before = false;
            let mut j: usize = 0;
            #[cfg_attr(verus_keep_ghost, verus_spec(
                invariant
                    count == records.len(),
                    i < count,
                    rec@ == records@[i as int]@,
                    j <= i,
                    seen_before == exists|m: int| 0 <= m < j && (#[trigger] records@[m])@ == records@[i as int]@,
                decreases i - j,
            ))]
            while j < i {
                if bytes_eq(records[j], rec) {
                    seen_before = true;
                }
                j = j.saturating_add(1);
            }
            mask.push(!seen_before);
            #[cfg(verus_keep_ghost)]
            proof! {
                let v = views(records@);
                assert(fresh(v, i as int) == !seen_before) by {
                    if seen_before {
                        let m = choose|m: int| 0 <= m < i && (#[trigger] records@[m])@ == records@[i as int]@;
                        assert(v[m] == v[i as int]);
                    } else {
                        assert forall|m: int| 0 <= m < i implies v[m] != v[i as int] by {
                            assert(v[m] == records@[m]@);
                        }
                    }
                }
            }
            i = i.saturating_add(1);
        }
        mask
    }
}

/// The rollback's checkout list (D-DATA-19): the patch's paths, then every non-empty changed path, each once in
/// first-seen order. Proved: the result is `dedup(touched ++ (changed minus empty records))` (N4), so it holds
/// every such path exactly once and no other.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        views(r@) == rollback_spec(views(touched@), views(changed@)),
))]
#[expect(
    clippy::indexing_slicing,
    reason = "verus: every index is proved in bounds"
)]
pub(crate) fn rollback_list<'a, S: SeenSet>(
    seen: &S,
    touched: &[&'a [u8]],
    changed: &[&'a [u8]],
) -> Vec<&'a [u8]> {
    let mut all: Vec<&'a [u8]> = Vec::new();
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            i <= touched.len(),
            views(all@) == views(touched@).take(i as int),
        decreases touched.len() - i,
    ))]
    while i < touched.len() {
        let p = touched[i];
        #[cfg(verus_keep_ghost)]
        proof_decl! {
            let ghost before = all@;
        }
        all.push(p);
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(views(all@) =~= views(before).push(p@));
            assert(views(touched@).take(i + 1) =~= views(touched@).take(i as int).push(p@));
        }
        i = i.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(views(touched@).take(touched.len() as int) =~= views(touched@));
        assert(views(changed@).take(0) =~= Seq::<Seq<u8>>::empty());
        assert(views(touched@) + drop_empty(views(changed@).take(0)) =~= views(touched@));
    }
    let mut j: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            j <= changed.len(),
            views(all@) == views(touched@) + drop_empty(views(changed@).take(j as int)),
        decreases changed.len() - j,
    ))]
    while j < changed.len() {
        let p = changed[j];
        #[cfg(verus_keep_ghost)]
        proof! {
            let c = views(changed@).take(j + 1);
            assert(c.drop_last() =~= views(changed@).take(j as int));
            assert(c.last() == p@);
        }
        if !p.is_empty() {
            #[cfg(verus_keep_ghost)]
            proof_decl! {
                let ghost before = views(all@);
            }
            all.push(p);
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(views(all@) =~= before.push(p@));
                let d = drop_empty(views(changed@).take(j as int));
                assert(views(touched@) + d.push(p@) =~= (views(touched@) + d).push(p@));
            }
        }
        j = j.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(views(changed@).take(changed.len() as int) =~= views(changed@));
    }
    let mask = seen.first_seen(all.as_slice());
    let mut out: Vec<&'a [u8]> = Vec::new();
    let mut k: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            k <= all.len(),
            mask@.len() == all@.len(),
            forall|m: int| 0 <= m < all@.len() ==> #[trigger] mask@[m] == fresh(views(all@), m),
            views(out@) == dedup(views(all@).take(k as int)),
        decreases all.len() - k,
    ))]
    while k < all.len() {
        let p = all[k];
        #[cfg(verus_keep_ghost)]
        proof! {
            let v = views(all@);
            let t = v.take(k + 1);
            assert(t.drop_last() =~= v.take(k as int));
            assert(t.last() == v[k as int]);
            assert(t.drop_last().contains(t.last()) == !fresh(v, k as int)) by {
                if !fresh(v, k as int) {
                    let m = choose|m: int| 0 <= m < k && v[m] == v[k as int];
                    assert(t.drop_last()[m] == t.last());
                }
            }
        }
        if mask.get(k) == Some(&true) {
            #[cfg(verus_keep_ghost)]
            proof_decl! {
                let ghost before = views(out@);
            }
            out.push(p);
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(views(out@) =~= before.push(p@));
            }
        }
        k = k.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(views(all@).take(all.len() as int) =~= views(all@));
    }
    out
}
