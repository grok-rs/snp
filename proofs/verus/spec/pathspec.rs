//! Spec of kernel K7, the D-DATA-19 pathspec lists (docs/VERIFICATION.md §9.2, ADR-093, ADR-095).
//!
//! The stash checks out only the files of its patch, and the conflict rollback those plus every file `git diff`
//! lists (D-DATA-19). Four pure steps decide which files git rewrites; each has an exact spec here:
//!
//! - `parse_from`: the paths of `git apply --numstat -z` (records `<added>\t<deleted>\t<path>\0`; git's
//!   `apply.c` `numstat_patch_list`, `write_name_quoted(…, '\0')`), as the shell's former `split`/`splitn` parser
//!   read them, including its two-name rename branch.
//! - `lit_list`: the `--pathspec-file-nul` list, `:(literal)<path>\0` per path.
//! - `chunks_ok`: the argument chunks of the git < 2.25 fallback (`checkout -- :(literal)<path>…`).
//! - `dedup`: the rollback list, the first-seen dedup of the patch's paths followed by the non-empty changed ones.
//!
//! Properties (N-numbers of §9.2):
//!
//! - N1 `lemma_numstat_complete`: git-shaped output parses to exactly its names, in order.
//! - N2 `lemma_numstat_wf` and `lemma_numstat_short_record`: every parsed path is a non-empty, NUL-free slice of
//!   the output; a header record with fewer than two tabs is an error. No panic is the kernel's own condition.
//! - N3 `lemma_lit_list_items` and `lemma_lit_list_injective`: git's NUL-item reader sees exactly
//!   `map(":(literal)" ++ ·, L)` and the encoding is injective on NUL-free lists; `lemma_chunks_concat`: the chunks
//!   of the fallback, concatenated, are the whole list.
//! - N4 `ok_path` (the sink's refusal, the kernel's `checkout_refusal` contract) and `lemma_dedup_props`.
use vstd::prelude::*;

verus! {

// ---------------------------------------------------------------------------------------------------------------
// Byte scans
/// The first NUL at or after `i`, or `s.len()` when there is none.
pub open spec fn nul_at(s: Seq<u8>, i: int) -> int
    decreases s.len() - i,
{
    if i >= s.len() {
        s.len() as int
    } else if s[i] == 0u8 {
        i
    } else {
        nul_at(s, i + 1)
    }
}

/// The first TAB in `s[i..e]`, or `e` when there is none.
pub open spec fn tab_in(s: Seq<u8>, i: int, e: int) -> int
    decreases e - i,
{
    if i >= e {
        e
    } else if s[i] == 9u8 {
        i
    } else {
        tab_in(s, i + 1, e)
    }
}

/// `s[i..e]` holds no NUL.
pub open spec fn nul_free_in(s: Seq<u8>, i: int, e: int) -> bool {
    forall|j: int| i <= j < e ==> s[j] != 0u8
}

/// `p` holds no NUL.
pub open spec fn nul_free(p: Seq<u8>) -> bool {
    forall|j: int| 0 <= j < p.len() ==> p[j] != 0u8
}

/// `p` holds no TAB.
pub open spec fn tab_free(p: Seq<u8>) -> bool {
    forall|j: int| 0 <= j < p.len() ==> p[j] != 9u8
}

/// `nul_at` is the first NUL at or after `i`.
pub proof fn lemma_nul_at(s: Seq<u8>, i: int)
    requires
        0 <= i <= s.len(),
    ensures
        i <= nul_at(s, i) <= s.len(),
        nul_free_in(s, i, nul_at(s, i)),
        nul_at(s, i) < s.len() ==> s[nul_at(s, i)] == 0u8,
    decreases s.len() - i,
{
    if i < s.len() && s[i] != 0u8 {
        lemma_nul_at(s, i + 1);
    }
}

/// A NUL-free stretch ending at a NUL (or at the end) is where `nul_at` stops.
pub proof fn lemma_nul_at_is(s: Seq<u8>, i: int, k: int)
    requires
        0 <= i <= k <= s.len(),
        nul_free_in(s, i, k),
        k < s.len() ==> s[k] == 0u8,
    ensures
        nul_at(s, i) == k,
    decreases k - i,
{
    if i < k {
        lemma_nul_at_is(s, i + 1, k);
    }
}

/// `tab_in` is the first TAB of `s[i..e]`.
pub proof fn lemma_tab_in(s: Seq<u8>, i: int, e: int)
    requires
        0 <= i <= e <= s.len(),
    ensures
        i <= tab_in(s, i, e) <= e,
        forall|j: int| i <= j < tab_in(s, i, e) ==> s[j] != 9u8,
        tab_in(s, i, e) < e ==> s[tab_in(s, i, e)] == 9u8,
    decreases e - i,
{
    if i < e && s[i] != 9u8 {
        lemma_tab_in(s, i + 1, e);
    }
}

/// A TAB-free stretch ending at a TAB (or at `e`) is where `tab_in` stops.
pub proof fn lemma_tab_in_is(s: Seq<u8>, i: int, t: int, e: int)
    requires
        0 <= i <= t <= e <= s.len(),
        forall|j: int| i <= j < t ==> s[j] != 9u8,
        t < e ==> s[t] == 9u8,
    ensures
        tab_in(s, i, e) == t,
    decreases t - i,
{
    if i < t {
        lemma_tab_in_is(s, i + 1, t, e);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// git apply --numstat -z
/// `Ok(head ++ t)` for `Ok(t)`; an error stays.
pub open spec fn prepend(head: Seq<(int, int)>, r: Result<Seq<(int, int)>, (int, int)>) -> Result<
    Seq<(int, int)>,
    (int, int),
> {
    match r {
        Ok(t) => Ok(head + t),
        Err(e) => Err(e),
    }
}

/// The spans of the paths in the records from byte `i` on, or `Err(span)` of the first malformed record.
///
/// Records end at a NUL (the last may end at the end of the output); an empty record is skipped. A record's
/// path is what follows its second TAB (TABs and newlines in the path are kept). A record without two TABs is an
/// error. An empty path announces a rename or copy: the next two records are its names, and both must exist and be
/// non-empty. The `k < i`-style guards are never taken (`lemma_nul_at`); they only make termination evident.
pub open spec fn parse_from(out: Seq<u8>, i: int) -> Result<Seq<(int, int)>, (int, int)>
    decreases out.len() - i,
{
    if i < 0 || i > out.len() {
        Ok(Seq::empty())
    } else {
        let k = nul_at(out, i);
        if k < i || k > out.len() {
            Ok(Seq::empty())
        } else if k == i {
            if k >= out.len() {
                Ok(Seq::empty())
            } else {
                parse_from(out, k + 1)
            }
        } else {
            let t1 = tab_in(out, i, k);
            let t2 = if t1 < k {
                tab_in(out, t1 + 1, k)
            } else {
                k
            };
            if t2 >= k {
                Err((i, k))
            } else if t2 + 1 < k {
                prepend(
                    seq![(t2 + 1, k)],
                    if k >= out.len() {
                        Ok(Seq::empty())
                    } else {
                        parse_from(out, k + 1)
                    },
                )
            } else if k >= out.len() {
                Err((i, k))
            } else {
                let k2 = nul_at(out, k + 1);
                if k2 <= k + 1 || k2 >= out.len() {
                    Err((i, k))
                } else {
                    let k3 = nul_at(out, k2 + 1);
                    if k3 <= k2 + 1 || k3 > out.len() {
                        Err((i, k))
                    } else {
                        prepend(
                            seq![(k + 1, k2), (k2 + 1, k3)],
                            if k3 >= out.len() {
                                Ok(Seq::empty())
                            } else {
                                parse_from(out, k3 + 1)
                            },
                        )
                    }
                }
            }
        }
    }
}

/// The spans of `v` as integers.
pub open spec fn ints(v: Seq<(usize, usize)>) -> Seq<(int, int)> {
    Seq::new(v.len(), |k: int| (v[k].0 as int, v[k].1 as int))
}

/// The contract of `numstat_spans`: its result is `parse_from(out, 0)`.
pub open spec fn numstat_post(
    out: Seq<u8>,
    r: Result<Vec<(usize, usize)>, (usize, usize)>,
) -> bool {
    match r {
        Ok(v) => parse_from(out, 0) == Ok::<Seq<(int, int)>, (int, int)>(ints(v@)),
        Err(e) => parse_from(out, 0) == Err::<Seq<(int, int)>, (int, int)>(
            (e.0 as int, e.1 as int),
        ),
    }
}

/// The bytes the spans select.
pub open spec fn names(out: Seq<u8>, sp: Seq<(int, int)>) -> Seq<Seq<u8>> {
    Seq::new(sp.len(), |k: int| out.subrange(sp[k].0, sp[k].1))
}

pub proof fn lemma_prepend_cat(
    a: Seq<(int, int)>,
    b: Seq<(int, int)>,
    r: Result<Seq<(int, int)>, (int, int)>,
)
    ensures
        prepend(a, prepend(b, r)) == prepend(a + b, r),
{
    if let Ok(t) = r {
        assert(a + (b + t) =~= (a + b) + t);
    }
}

/// Every span `parse_from` returns is a non-empty, NUL-free stretch of `out` at or after `i`.
pub open spec fn spans_wf(out: Seq<u8>, i: int, sp: Seq<(int, int)>) -> bool {
    forall|k: int|
        0 <= k < sp.len() ==> {
            &&& i <= (#[trigger] sp[k]).0 < sp[k].1 <= out.len()
            &&& nul_free_in(out, sp[k].0, sp[k].1)
        }
}

/// N2: on `Ok`, every path is a non-empty, NUL-free stretch of the output.
pub proof fn lemma_numstat_wf(out: Seq<u8>, i: int)
    requires
        0 <= i <= out.len(),
    ensures
        parse_from(out, i) matches Ok(sp) ==> spans_wf(out, i, sp),
    decreases out.len() - i,
{
    let k = nul_at(out, i);
    lemma_nul_at(out, i);
    if k == i {
        if k < out.len() {
            lemma_numstat_wf(out, k + 1);
        }
    } else {
        let t1 = tab_in(out, i, k);
        lemma_tab_in(out, i, k);
        let t2 = if t1 < k {
            tab_in(out, t1 + 1, k)
        } else {
            k
        };
        if t1 < k {
            lemma_tab_in(out, t1 + 1, k);
        }
        if t2 < k && t2 + 1 < k {
            if k < out.len() {
                lemma_numstat_wf(out, k + 1);
            }
        } else if t2 < k && k < out.len() {
            let k2 = nul_at(out, k + 1);
            lemma_nul_at(out, k + 1);
            if k + 1 < k2 < out.len() {
                let k3 = nul_at(out, k2 + 1);
                lemma_nul_at(out, k2 + 1);
                if k2 + 1 < k3 && k3 < out.len() {
                    lemma_numstat_wf(out, k3 + 1);
                }
            }
        }
    }
}

/// N2: a non-empty header record with fewer than two TABs is an error naming that record.
pub proof fn lemma_numstat_short_record(out: Seq<u8>, i: int)
    requires
        0 <= i <= out.len(),
        nul_at(out, i) > i,
        tab_in(out, i, nul_at(out, i)) >= nul_at(out, i) || tab_in(
            out,
            tab_in(out, i, nul_at(out, i)) + 1,
            nul_at(out, i),
        ) >= nul_at(out, i),
    ensures
        parse_from(out, i) == Err::<Seq<(int, int)>, (int, int)>((i, nul_at(out, i))),
{
    lemma_nul_at(out, i);
}

/// One `numstat -z` record of a modified file: `<added>\t<deleted>\t<name>\0`.
pub open spec fn numstat_rec(a: Seq<u8>, d: Seq<u8>, name: Seq<u8>) -> Seq<u8> {
    a + seq![9u8] + d + seq![9u8] + name + seq![0u8]
}

/// The output of `git apply --numstat -z` for the records `recs` (`(added, deleted, name)` each), in order.
pub open spec fn numstat_out(recs: Seq<(Seq<u8>, Seq<u8>, Seq<u8>)>) -> Seq<u8>
    decreases recs.len(),
{
    if recs.len() == 0 {
        Seq::empty()
    } else {
        numstat_rec(recs[0].0, recs[0].1, recs[0].2) + numstat_out(recs.drop_first())
    }
}

/// The shape git prints: counts without TAB or NUL (digits, or `-` for a binary file), a non-empty name without
/// NUL (TABs and newlines allowed).
pub open spec fn rec_wf(r: (Seq<u8>, Seq<u8>, Seq<u8>)) -> bool {
    &&& tab_free(r.0) && nul_free(r.0)
    &&& tab_free(r.1) && nul_free(r.1)
    &&& r.2.len() > 0 && nul_free(r.2)
}

/// The names of the records.
pub open spec fn rec_names(recs: Seq<(Seq<u8>, Seq<u8>, Seq<u8>)>) -> Seq<Seq<u8>> {
    Seq::new(recs.len(), |k: int| recs[k].2)
}

/// One well-formed record at `p` parses to its name and continues after its NUL.
proof fn lemma_numstat_one(out: Seq<u8>, p: int, a: Seq<u8>, d: Seq<u8>, name: Seq<u8>)
    requires
        0 <= p,
        rec_wf((a, d, name)),
        p + numstat_rec(a, d, name).len() <= out.len(),
        out.subrange(p, p + numstat_rec(a, d, name).len()) == numstat_rec(a, d, name),
    ensures
        ({
            let k = p + numstat_rec(a, d, name).len() - 1;
            &&& k < out.len()
            &&& parse_from(out, p) == prepend(
                seq![(p + a.len() + d.len() + 2, k)],
                parse_from(out, k + 1),
            )
            &&& out.subrange(p + a.len() + d.len() + 2, k) == name
        }),
{
    let rec = numstat_rec(a, d, name);
    let k = p + rec.len() - 1;
    let t1 = p + a.len();
    let t2 = t1 + 1 + d.len();
    assert forall|j: int| 0 <= j < rec.len() implies out[p + j] == rec[j] by {
        assert(out.subrange(p, p + rec.len())[j] == out[p + j]);
    }
    assert forall|j: int| 0 <= j < a.len() implies rec[j] == a[j] by {}
    assert(rec[a.len() as int] == 9u8);
    assert forall|j: int| 0 <= j < d.len() implies rec[a.len() + 1 + j] == d[j] by {}
    assert(rec[(a.len() + 1 + d.len()) as int] == 9u8);
    assert forall|j: int| 0 <= j < name.len() implies rec[a.len() + d.len() + 2 + j]
        == name[j] by {}
    assert(rec[rec.len() - 1] == 0u8);
    assert(nul_free_in(out, p, k)) by {
        assert forall|j: int| p <= j < k implies out[j] != 0u8 by {
            let o = j - p;
            assert(out[j] == rec[o]);
            if o < a.len() {
            } else if o == a.len() {
            } else if o < a.len() + 1 + d.len() {
                assert(rec[o] == d[o - a.len() - 1]);
            } else if o == a.len() + 1 + d.len() {
            } else {
                assert(rec[o] == name[o - a.len() - d.len() - 2]);
            }
        }
    }
    assert(out[k] == rec[rec.len() - 1]);
    lemma_nul_at_is(out, p, k);
    assert forall|j: int| p <= j < t1 implies out[j] != 9u8 by {
        assert(out[j] == rec[j - p]);
    }
    assert(out[t1] == rec[a.len() as int]);
    lemma_tab_in_is(out, p, t1, k);
    assert forall|j: int| t1 + 1 <= j < t2 implies out[j] != 9u8 by {
        assert(out[j] == rec[j - p]);
        assert(rec[j - p] == d[j - p - a.len() - 1]);
    }
    assert(out[t2] == rec[(a.len() + 1 + d.len()) as int]);
    lemma_tab_in_is(out, t1 + 1, t2, k);
    assert(out.subrange(t2 + 1, k) =~= name) by {
        assert forall|j: int| 0 <= j < name.len() implies out.subrange(t2 + 1, k)[j] == name[j] by {
            assert(out[t2 + 1 + j] == rec[t2 + 1 + j - p]);
        }
    }
}

/// N1 at an offset: after a prefix `pre`, the records parse to their names, shifted by `|pre|`.
proof fn lemma_numstat_complete_at(pre: Seq<u8>, recs: Seq<(Seq<u8>, Seq<u8>, Seq<u8>)>)
    requires
        forall|k: int| 0 <= k < recs.len() ==> rec_wf(#[trigger] recs[k]),
    ensures
        ({
            let out = pre + numstat_out(recs);
            parse_from(out, pre.len() as int) matches Ok(sp) && names(out, sp) == rec_names(recs)
        }),
    decreases recs.len(),
{
    let out = pre + numstat_out(recs);
    if recs.len() == 0 {
        assert(out =~= pre);
        assert(nul_at(out, pre.len() as int) == out.len());
        assert(names(out, Seq::empty()) =~= rec_names(recs));
    } else {
        let r0 = recs[0];
        let rest = recs.drop_first();
        let rec = numstat_rec(r0.0, r0.1, r0.2);
        let p = pre.len() as int;
        let pre2 = pre + rec;
        assert(out =~= pre2 + numstat_out(rest));
        assert(out.subrange(p, p + rec.len()) =~= rec);
        assert(rec_wf(recs[0]));
        lemma_numstat_one(out, p, r0.0, r0.1, r0.2);
        assert forall|k: int| 0 <= k < rest.len() implies rec_wf(#[trigger] rest[k]) by {
            assert(rest[k] == recs[k + 1]);
        }
        lemma_numstat_complete_at(pre2, rest);
        let k = p + rec.len() - 1;
        let first = (p + r0.0.len() + r0.1.len() + 2, k);
        // the rest parses from k + 1 == |pre2|
        assert(pre2.len() == k + 1);
        let tsp = parse_from(out, k + 1)->Ok_0;
        let sp = seq![first] + tsp;
        assert(parse_from(out, p) == Ok::<Seq<(int, int)>, (int, int)>(sp));
        assert(names(out, tsp).len() == rec_names(rest).len());
        assert(names(out, sp) =~= rec_names(recs)) by {
            assert forall|j: int| 0 <= j < sp.len() implies #[trigger] names(out, sp)[j]
                == rec_names(recs)[j] by {
                assert(names(out, sp)[j] == out.subrange(sp[j].0, sp[j].1));
                assert(rec_names(recs)[j] == recs[j].2);
                if j > 0 {
                    assert(sp[j] == tsp[j - 1]);
                    assert(names(out, tsp)[j - 1] == out.subrange(tsp[j - 1].0, tsp[j - 1].1));
                    assert(names(out, tsp)[j - 1] == rec_names(rest)[j - 1]);
                    assert(rec_names(rest)[j - 1] == rest[j - 1].2);
                    assert(rest[j - 1] == recs[j]);
                } else {
                    assert(sp[0] == first);
                }
            }
        }
    }
}

/// N1: the output git prints for well-formed records parses to exactly their names, in order.
pub proof fn lemma_numstat_complete(recs: Seq<(Seq<u8>, Seq<u8>, Seq<u8>)>)
    requires
        forall|k: int| 0 <= k < recs.len() ==> rec_wf(#[trigger] recs[k]),
    ensures
        parse_from(numstat_out(recs), 0) matches Ok(sp) && names(numstat_out(recs), sp)
            == rec_names(recs),
{
    lemma_numstat_complete_at(Seq::empty(), recs);
    assert(Seq::<u8>::empty() + numstat_out(recs) =~= numstat_out(recs));
}

// ---------------------------------------------------------------------------------------------------------------
// :(literal) pathspecs
/// The byte views of the paths.
pub open spec fn views(ps: Seq<&[u8]>) -> Seq<Seq<u8>> {
    Seq::new(ps.len(), |k: int| ps[k]@)
}

/// `:(literal)`
pub open spec fn magic() -> Seq<u8> {
    seq![58u8, 40u8, 108u8, 105u8, 116u8, 101u8, 114u8, 97u8, 108u8, 41u8]
}

/// `:(literal)` holds no NUL.
pub proof fn lemma_magic()
    ensures
        magic().len() == 10,
        nul_free(magic()),
{
    let m = magic();
    assert(m[0] == 58u8);
    assert(m[1] == 40u8);
    assert(m[2] == 108u8);
    assert(m[3] == 105u8);
    assert(m[4] == 116u8);
    assert(m[5] == 101u8);
    assert(m[6] == 114u8);
    assert(m[7] == 97u8);
    assert(m[8] == 108u8);
    assert(m[9] == 41u8);
}

/// `:(literal)<p>`: one literal pathspec.
pub open spec fn lit(p: Seq<u8>) -> Seq<u8> {
    magic() + p
}

/// `:(literal)<p>\0` for each path, in order: the `--pathspec-file-nul` list.
pub open spec fn lit_list(l: Seq<Seq<u8>>) -> Seq<u8>
    decreases l.len(),
{
    if l.len() == 0 {
        Seq::empty()
    } else {
        lit_list(l.drop_last()) + lit(l.last()) + seq![0u8]
    }
}

/// The items git's `--pathspec-file-nul` reader takes from `s`: each up to the next NUL; a final item without a
/// NUL counts, an empty input has none.
pub open spec fn nul_items(s: Seq<u8>) -> Seq<Seq<u8>>
    decreases s.len(),
{
    if s.len() == 0 {
        Seq::empty()
    } else {
        let k = nul_at(s, 0);
        if k < 0 || k >= s.len() {
            seq![s]
        } else {
            seq![s.subrange(0, k)] + nul_items(s.subrange(k + 1, s.len() as int))
        }
    }
}

/// Every path is non-empty and NUL-free: what the checkout sink accepts (an empty `:(literal)` selects the whole
/// tree, a NUL would split an item).
pub open spec fn ok_path(p: Seq<u8>) -> bool {
    p.len() > 0 && nul_free(p)
}

/// The literal pathspecs of the paths.
pub open spec fn lit_items(l: Seq<Seq<u8>>) -> Seq<Seq<u8>> {
    Seq::new(l.len(), |k: int| lit(l[k]))
}

/// The items of a NUL-terminated (or empty) prefix and of what follows add up.
proof fn lemma_nul_items_append(a: Seq<u8>, b: Seq<u8>)
    requires
        a.len() == 0 || a.last() == 0u8,
    ensures
        nul_items(a + b) == nul_items(a) + nul_items(b),
    decreases a.len(),
{
    if a.len() == 0 {
        assert(a + b =~= b);
        assert(nul_items(a) + nul_items(b) =~= nul_items(b));
    } else {
        let s = a + b;
        lemma_nul_at(a, 0);
        let k = nul_at(a, 0);
        assert(k < a.len()) by {
            if k >= a.len() {
                assert(nul_free_in(a, 0, a.len() as int));
                assert(a[a.len() - 1] == 0u8);
            }
        }
        assert forall|j: int| 0 <= j < k implies s[j] != 0u8 by {
            assert(s[j] == a[j]);
        }
        assert(s[k] == a[k]);
        lemma_nul_at_is(s, 0, k);
        let a2 = a.subrange(k + 1, a.len() as int);
        assert(s.subrange(0, k) =~= a.subrange(0, k));
        assert(s.subrange(k + 1, s.len() as int) =~= a2 + b);
        assert(a2.len() == 0 || a2.last() == 0u8);
        lemma_nul_items_append(a2, b);
        assert(nul_items(a) == seq![a.subrange(0, k)] + nul_items(a2));
        assert(nul_items(s) =~= nul_items(a) + nul_items(b));
    }
}

/// N3: for NUL-free paths, git reads exactly one `:(literal)<p>` item per path, in order.
pub proof fn lemma_lit_list_items(l: Seq<Seq<u8>>)
    requires
        forall|k: int| 0 <= k < l.len() ==> nul_free(#[trigger] l[k]),
    ensures
        nul_items(lit_list(l)) == lit_items(l),
    decreases l.len(),
{
    if l.len() == 0 {
        assert(lit_items(l) =~= Seq::<Seq<u8>>::empty());
    } else {
        let init = l.drop_last();
        assert forall|k: int| 0 <= k < init.len() implies nul_free(#[trigger] init[k]) by {
            assert(init[k] == l[k]);
        }
        lemma_lit_list_items(init);
        let head = lit_list(init);
        let x = lit(l.last()) + seq![0u8];
        assert(head.len() == 0 || head.last() == 0u8);
        assert(lit_list(l) =~= head + x);
        lemma_nul_items_append(head, x);
        assert(nul_free(l[l.len() - 1]));
        let k = lit(l.last()).len() as int;
        lemma_magic();
        assert forall|j: int| 0 <= j < k implies x[j] != 0u8 by {
            if j >= 10 {
                assert(x[j] == l.last()[j - 10]);
            } else {
                assert(x[j] == magic()[j]);
            }
        }
        lemma_nul_at_is(x, 0, k);
        assert(x.subrange(0, k) =~= lit(l.last()));
        assert(x.subrange(k + 1, x.len() as int) =~= Seq::<u8>::empty());
        assert(nul_items(x.subrange(k + 1, x.len() as int)) =~= Seq::<Seq<u8>>::empty());
        assert(nul_items(x) =~= seq![lit(l.last())]);
        assert(lit_items(l) =~= lit_items(init) + seq![lit(l.last())]);
    }
}

/// N3: the encoding is injective on NUL-free lists.
pub proof fn lemma_lit_list_injective(l1: Seq<Seq<u8>>, l2: Seq<Seq<u8>>)
    requires
        forall|k: int| 0 <= k < l1.len() ==> nul_free(#[trigger] l1[k]),
        forall|k: int| 0 <= k < l2.len() ==> nul_free(#[trigger] l2[k]),
        lit_list(l1) == lit_list(l2),
    ensures
        l1 == l2,
{
    lemma_lit_list_items(l1);
    lemma_lit_list_items(l2);
    assert(lit_items(l1) == lit_items(l2));
    assert(l1.len() == lit_items(l1).len());
    assert forall|k: int| 0 <= k < l1.len() implies l1[k] == l2[k] by {
        assert(lit_items(l1)[k] == lit_items(l2)[k]);
        assert(lit(l1[k]).subrange(10, lit(l1[k]).len() as int) =~= l1[k]);
        assert(lit(l2[k]).subrange(10, lit(l2[k]).len() as int) =~= l2[k]);
    }
    assert(l1 =~= l2);
}

// ---------------------------------------------------------------------------------------------------------------
// The git < 2.25 argument chunks
/// `Σ (|l[j]| + 11)` over `s..e`: the bytes of the literal pathspecs `l[s..e]` with one separator after each.
pub open spec fn lit_sum(l: Seq<Seq<u8>>, s: int, e: int) -> int
    decreases e - s,
{
    if e <= s {
        0
    } else {
        lit_sum(l, s, e - 1) + l[e - 1].len() + 11
    }
}

/// The chunks tile `0..|l|` with non-empty, contiguous, in-order ranges (none for an empty list); a chunk of more
/// than one path keeps its arguments and the separators between them within `budget`; every chunk but the last
/// could not take its next path (greedy).
pub open spec fn chunks_ok(r: Seq<(usize, usize)>, l: Seq<Seq<u8>>, budget: int) -> bool {
    &&& l.len() == 0 ==> r.len() == 0
    &&& l.len() > 0 ==> r.len() >= 1 && r[0].0 == 0 && r.last().1 == l.len()
    &&& forall|k: int| 0 <= k < r.len() ==> (#[trigger] r[k]).0 < r[k].1
    &&& forall|k: int| 0 <= k < r.len() - 1 ==> (#[trigger] r[k]).1 == r[k + 1].0
    &&& forall|k: int|
        0 <= k < r.len() ==> (#[trigger] r[k]).1 - r[k].0 == 1 || lit_sum(
            l,
            r[k].0 as int,
            r[k].1 as int,
        ) <= budget + 1
    &&& forall|k: int|
        0 <= k < r.len() - 1 ==> lit_sum(l, (#[trigger] r[k]).0 as int, r[k].1 + 1) > budget + 1
}

/// The chunks' items, concatenated in order.
pub open spec fn chunk_concat<T>(l: Seq<T>, r: Seq<(usize, usize)>) -> Seq<T>
    decreases r.len(),
{
    if r.len() == 0 {
        Seq::empty()
    } else {
        chunk_concat(l, r.drop_last()) + l.subrange(r.last().0 as int, r.last().1 as int)
    }
}

/// Contiguous ranges from 0 concatenate to the prefix they cover.
proof fn lemma_chunk_concat_prefix<T>(l: Seq<T>, r: Seq<(usize, usize)>)
    requires
        r.len() >= 1,
        r[0].0 == 0,
        r.last().1 <= l.len(),
        forall|k: int| 0 <= k < r.len() ==> (#[trigger] r[k]).0 <= r[k].1,
        forall|k: int| 0 <= k < r.len() - 1 ==> (#[trigger] r[k]).1 == r[k + 1].0,
    ensures
        chunk_concat(l, r) == l.subrange(0, r.last().1 as int),
    decreases r.len(),
{
    if r.len() == 1 {
        assert(r.drop_last() =~= Seq::<(usize, usize)>::empty());
        assert(chunk_concat(l, r.drop_last()) =~= Seq::<T>::empty());
        assert(Seq::<T>::empty() + l.subrange(0, r[0].1 as int) =~= l.subrange(0, r[0].1 as int));
    } else {
        let init = r.drop_last();
        assert(init[0] == r[0]);
        assert(init.last() == r[r.len() - 2]);
        assert forall|k: int| 0 <= k < init.len() implies (#[trigger] init[k]).0 <= init[k].1 by {
            assert(init[k] == r[k]);
        }
        assert forall|k: int| 0 <= k < init.len() - 1 implies (#[trigger] init[k]).1 == init[k
            + 1].0 by {
            assert(init[k] == r[k]);
            assert(init[k + 1] == r[k + 1]);
        }
        assert(r[r.len() - 2].1 == r[r.len() - 1].0);
        assert(r[r.len() - 2].0 <= r[r.len() - 2].1);
        lemma_chunk_concat_prefix(l, init);
        assert(l.subrange(0, init.last().1 as int) + l.subrange(
            r.last().0 as int,
            r.last().1 as int,
        ) =~= l.subrange(0, r.last().1 as int));
    }
}

/// N3 for the chunked form: the chunks, concatenated, are the whole list (of paths, or of anything indexed like it,
/// such as their literal pathspecs).
pub proof fn lemma_chunks_concat<T>(
    l: Seq<Seq<u8>>,
    r: Seq<(usize, usize)>,
    budget: int,
    items: Seq<T>,
)
    requires
        chunks_ok(r, l, budget),
        items.len() == l.len(),
    ensures
        chunk_concat(items, r) == items,
{
    if l.len() == 0 {
        assert(chunk_concat(items, r) =~= items);
    } else {
        lemma_chunk_concat_prefix(items, r);
        assert(items.subrange(0, items.len() as int) =~= items);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The rollback list
/// `xs[i]` does not occur before `i`.
pub open spec fn fresh(xs: Seq<Seq<u8>>, i: int) -> bool {
    forall|j: int| 0 <= j < i ==> xs[j] != xs[i]
}

/// `xs` without its empty records.
pub open spec fn drop_empty(xs: Seq<Seq<u8>>) -> Seq<Seq<u8>>
    decreases xs.len(),
{
    if xs.len() == 0 {
        Seq::empty()
    } else if xs.last().len() == 0 {
        drop_empty(xs.drop_last())
    } else {
        drop_empty(xs.drop_last()).push(xs.last())
    }
}

/// The first occurrence of each record, in first-seen order.
pub open spec fn dedup(xs: Seq<Seq<u8>>) -> Seq<Seq<u8>>
    decreases xs.len(),
{
    if xs.len() == 0 {
        Seq::empty()
    } else if xs.drop_last().contains(xs.last()) {
        dedup(xs.drop_last())
    } else {
        dedup(xs.drop_last()).push(xs.last())
    }
}

/// The rollback list (N4): the first-seen dedup of the patch's paths followed by the non-empty changed paths.
pub open spec fn rollback_spec(touched: Seq<Seq<u8>>, changed: Seq<Seq<u8>>) -> Seq<Seq<u8>> {
    dedup(touched + drop_empty(changed))
}

/// `drop_empty` keeps exactly the non-empty records.
pub proof fn lemma_drop_empty(xs: Seq<Seq<u8>>)
    ensures
        forall|x: Seq<u8>|
            #![trigger drop_empty(xs).contains(x)]
            #![trigger xs.contains(x)]
            drop_empty(xs).contains(x) <==> (xs.contains(x) && x.len() > 0),
    decreases xs.len(),
{
    if xs.len() > 0 {
        let init = xs.drop_last();
        lemma_drop_empty(init);
        assert forall|x: Seq<u8>|
            #![trigger drop_empty(xs).contains(x)]
            #![trigger xs.contains(x)]
            drop_empty(xs).contains(x) <==> (xs.contains(x) && x.len() > 0) by {
            if xs.contains(x) && x.len() > 0 {
                let j = choose|j: int| 0 <= j < xs.len() && xs[j] == x;
                if j < xs.len() - 1 {
                    assert(init[j] == x);
                    assert(init.contains(x));
                }
            }
            if init.contains(x) {
                let j = choose|j: int| 0 <= j < init.len() && init[j] == x;
                assert(xs[j] == x);
            }
            if drop_empty(xs).contains(x) && xs.last().len() > 0 && !drop_empty(init).contains(x) {
                let j = choose|j: int| 0 <= j < drop_empty(xs).len() && drop_empty(xs)[j] == x;
                assert(j == drop_empty(init).len() as int) by {
                    if j < drop_empty(init).len() {
                        assert(drop_empty(init)[j] == x);
                    }
                }
                assert(xs[xs.len() - 1] == x);
            }
            if xs.last().len() > 0 {
                let d = drop_empty(init);
                assert(d.push(xs.last())[d.len() as int] == xs.last());
                assert forall|y: Seq<u8>| #[trigger] d.contains(y) implies d.push(
                    xs.last(),
                ).contains(y) by {
                    let j = choose|j: int| 0 <= j < d.len() && d[j] == y;
                    assert(d.push(xs.last())[j] == y);
                }
            }
        }
    }
}

/// The rollback list neither loses nor repeats a path: it holds each record of `xs` exactly once.
pub proof fn lemma_dedup_props(xs: Seq<Seq<u8>>)
    ensures
        forall|x: Seq<u8>|
            #![trigger dedup(xs).contains(x)]
            #![trigger xs.contains(x)]
            dedup(xs).contains(x) <==> xs.contains(x),
        dedup(xs).no_duplicates(),
    decreases xs.len(),
{
    if xs.len() > 0 {
        let init = xs.drop_last();
        let x0 = xs.last();
        lemma_dedup_props(init);
        let d = dedup(init);
        assert forall|x: Seq<u8>|
            #![trigger dedup(xs).contains(x)]
            #![trigger xs.contains(x)]
            dedup(xs).contains(x) <==> xs.contains(x) by {
            if init.contains(x) {
                let j = choose|j: int| 0 <= j < init.len() && init[j] == x;
                assert(xs[j] == x);
            }
            if xs.contains(x) {
                let j = choose|j: int| 0 <= j < xs.len() && xs[j] == x;
                if j < xs.len() - 1 {
                    assert(init[j] == x);
                }
            }
            if !init.contains(x0) {
                assert(d.push(x0)[d.len() as int] == x0);
                if d.contains(x) {
                    let j = choose|j: int| 0 <= j < d.len() && d[j] == x;
                    assert(d.push(x0)[j] == x);
                }
                if d.push(x0).contains(x) && x != x0 {
                    let j = choose|j: int| 0 <= j < d.push(x0).len() && d.push(x0)[j] == x;
                    assert(d[j] == x);
                }
            }
        }
        if !init.contains(x0) {
            assert(!d.contains(x0));
            assert forall|i: int, j: int|
                0 <= i < d.push(x0).len() && 0 <= j < d.push(x0).len() && i != j implies d.push(
                x0,
            )[i] != d.push(x0)[j] by {
                if i < d.len() && j < d.len() {
                } else if i < d.len() {
                    assert(d.push(x0)[i] == d[i]);
                } else if j < d.len() {
                    assert(d.push(x0)[j] == d[j]);
                }
            }
        }
    }
}

} // verus!
