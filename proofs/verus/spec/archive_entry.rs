//! Spec of kernel K2, the archive entry-name check (docs/VERIFICATION.md §9.1, ADR-095, ADR-077).
//!
//! Upstream: pre-commit 4.7.0 has no check at all (`tarfile.extractall`/`zipfile.extractall` without a filter in
//! `golang.py:108-110` and `ruby.py:81-84`, D-SEC-08), so the spec is snp's own refusal rule of ADR-077: a name is
//! split on `/` (and `\` when `windows`), empty and `.` components are dropped, and the name is refused when it is
//! empty, rooted (`/x`, `\x`, `C:…` on every OS), has a `..` component or contains a NUL byte.
//!
//! Properties (P-numbers of §9.1): the kernel's verification condition is P0 (total, no panic, no overflow);
//! `entry_post` states P1 (the exact `Err` characterisation `refused`), P2 (the spans spell `comps_w(raw)`, in
//! bounds and strictly increasing) and P3 (every span and every component is `plain`). P4 (validation precedes
//! stripping) holds because the kernel takes no strip count: its `Err` depends on the whole name, and the shell
//! strips only an `Ok` result. Containment comes from P1 and P3: an accepted name has no `..` component, no
//! separator and no NUL. P5 (`lemma_normpath_append_plain`) adds that normalising leaves appended plain components
//! unchanged; it never reaches the `..` branch of `norm_step`. `lemma_normpath_dotdot` is the one lemma that does:
//! a `..` after a normalised directory drops its last component, which is why the kernel refuses `..` (two spec
//! mutants of that branch are registered).
use vstd::prelude::*;

verus! {

/// `b` separates components: `/`, and `\` too when `windows`.
pub open spec fn sep(b: u8, windows: bool) -> bool {
    b == 0x2fu8 || (windows && b == 0x5cu8)
}

/// `[A-Za-z]`.
pub open spec fn ascii_alpha(b: u8) -> bool {
    (0x41u8 <= b <= 0x5au8) || (0x61u8 <= b <= 0x7au8)
}

/// `s` starts like an absolute or drive-prefixed path on any OS: `/x`, `\x`, `C:…`.
pub open spec fn rooted(s: Seq<u8>) -> bool {
    ||| (s.len() > 0 && (s[0] == 0x2fu8 || s[0] == 0x5cu8))
    ||| (s.len() >= 2 && s[1] == 0x3au8 && ascii_alpha(s[0]))
}

/// `s` contains a NUL byte.
pub open spec fn has_nul(s: Seq<u8>) -> bool {
    exists|j: int| 0 <= j < s.len() && s[j] == 0u8
}

/// The component `.`.
pub open spec fn is_dot(c: Seq<u8>) -> bool {
    c.len() == 1 && c[0] == 0x2eu8
}

/// The component `..`.
pub open spec fn is_dotdot(c: Seq<u8>) -> bool {
    c.len() == 2 && c[0] == 0x2eu8 && c[1] == 0x2eu8
}

/// A component that survives the filter of `comps_w`: neither empty nor `.`.
pub open spec fn is_kept(c: Seq<u8>) -> bool {
    c.len() > 0 && !is_dot(c)
}

/// `split(s, sep_w)`: the maximal separator-free fields of `s`, like Rust's `<[u8]>::split` and Python's
/// `bytes.split(b"/")`: `split(ε) = [ε]`, a separator opens a new empty field, any other byte extends the last one.
pub open spec fn split_w(s: Seq<u8>, windows: bool) -> Seq<Seq<u8>>
    decreases s.len(),
{
    if s.len() == 0 {
        seq![Seq::<u8>::empty()]
    } else {
        let p = split_w(s.drop_last(), windows);
        if sep(s.last(), windows) {
            p.push(Seq::<u8>::empty())
        } else {
            p.update(p.len() - 1, p.last().push(s.last()))
        }
    }
}

/// `[c ∈ cs | c ≠ ε ∧ c ≠ "."]`.
pub open spec fn keep(cs: Seq<Seq<u8>>) -> Seq<Seq<u8>>
    decreases cs.len(),
{
    if cs.len() == 0 {
        Seq::<Seq<u8>>::empty()
    } else {
        let p = keep(cs.drop_last());
        if is_kept(cs.last()) {
            p.push(cs.last())
        } else {
            p
        }
    }
}

/// `comps_w(raw)`: the components of an entry name, empty and `.` ones dropped.
pub open spec fn comps_w(s: Seq<u8>, windows: bool) -> Seq<Seq<u8>> {
    keep(split_w(s, windows))
}

/// P1: the names the kernel refuses (`raw = ε ∨ rooted(raw) ∨ ".." ∈ comps_w(raw) ∨ 0 ∈ raw`).
pub open spec fn refused(s: Seq<u8>, windows: bool) -> bool {
    ||| s.len() == 0
    ||| rooted(s)
    ||| has_nul(s)
    ||| exists|k: int|
        0 <= k < comps_w(s, windows).len() && is_dotdot(#[trigger] comps_w(s, windows)[k])
}

/// P3: a plain component: non-empty, not `.` or `..`, no NUL and no separator.
pub open spec fn plain_comp(c: Seq<u8>, windows: bool) -> bool {
    &&& is_kept(c)
    &&& !is_dotdot(c)
    &&& forall|j: int| 0 <= j < c.len() ==> #[trigger] c[j] != 0u8 && !sep(c[j], windows)
}

/// P3 on a span `(start, end)` of `s`: in bounds and spelling a plain component.
pub open spec fn plain(s: Seq<u8>, c: (usize, usize), windows: bool) -> bool {
    &&& c.0 < c.1 <= s.len()
    &&& !is_dot(s.subrange(c.0 as int, c.1 as int))
    &&& !is_dotdot(s.subrange(c.0 as int, c.1 as int))
    &&& forall|j: int| c.0 <= j < c.1 ==> #[trigger] s[j] != 0u8 && !sep(s[j], windows)
}

/// `map(raw[s..e], spans)`.
pub open spec fn spans_view(s: Seq<u8>, spans: Seq<(usize, usize)>) -> Seq<Seq<u8>> {
    spans.map_values(|c: (usize, usize)| s.subrange(c.0 as int, c.1 as int))
}

/// P2 and P3 for an accepted name: the spans spell exactly `comps_w(s)`, are plain and strictly increasing, and
/// every component of `comps_w(s)` is plain.
pub open spec fn spans_ok(s: Seq<u8>, spans: Seq<(usize, usize)>, windows: bool) -> bool {
    &&& spans_view(s, spans) == comps_w(s, windows)
    &&& forall|k: int| 0 <= k < spans.len() ==> plain(s, #[trigger] spans[k], windows)
    &&& forall|k: int| 0 <= k < spans.len() - 1 ==> #[trigger] spans[k].1 < spans[k + 1].0
    &&& forall|k: int|
        0 <= k < comps_w(s, windows).len() ==> plain_comp(
            #[trigger] comps_w(s, windows)[k],
            windows,
        )
}

/// The finished fields of `s[..i]`: every field but the one still open at `i`.
pub open spec fn done(s: Seq<u8>, windows: bool, i: int) -> Seq<Seq<u8>> {
    split_w(s.subrange(0, i), windows).drop_last()
}

/// `a` is a prefix of `b`.
pub open spec fn is_prefix(a: Seq<Seq<u8>>, b: Seq<Seq<u8>>) -> bool {
    &&& a.len() <= b.len()
    &&& forall|k: int| 0 <= k < a.len() ==> #[trigger] a[k] == b[k]
}

/// `split_w` always has at least one field.
pub proof fn lemma_split_nonempty(s: Seq<u8>, windows: bool)
    ensures
        split_w(s, windows).len() >= 1,
    decreases s.len(),
{
    if s.len() > 0 {
        lemma_split_nonempty(s.drop_last(), windows);
    }
}

/// One more byte: a separator closes the open field, any other byte extends it.
pub proof fn lemma_split_step(s: Seq<u8>, windows: bool, i: int)
    requires
        0 <= i < s.len(),
    ensures
        split_w(s.subrange(0, i + 1), windows).len() >= 1,
        split_w(s.subrange(0, i), windows).len() >= 1,
        sep(s[i], windows) ==> split_w(s.subrange(0, i + 1), windows) == split_w(
            s.subrange(0, i),
            windows,
        ).push(Seq::<u8>::empty()),
        sep(s[i], windows) ==> done(s, windows, i + 1) == split_w(s.subrange(0, i), windows),
        !sep(s[i], windows) ==> done(s, windows, i + 1) == done(s, windows, i),
        !sep(s[i], windows) ==> split_w(s.subrange(0, i + 1), windows).last() == split_w(
            s.subrange(0, i),
            windows,
        ).last().push(s[i]),
{
    let a = s.subrange(0, i + 1);
    let b = s.subrange(0, i);
    assert(a.drop_last() =~= b);
    assert(a.last() == s[i]);
    lemma_split_nonempty(a, windows);
    lemma_split_nonempty(b, windows);
    let p = split_w(b, windows);
    if sep(s[i], windows) {
        assert(split_w(a, windows) == p.push(Seq::<u8>::empty()));
        assert(split_w(a, windows).drop_last() =~= p);
    } else {
        let q = p.update(p.len() - 1, p.last().push(s[i]));
        assert(split_w(a, windows) == q);
        assert(q.drop_last() =~= p.drop_last());
    }
}

/// `keep` of one more field.
pub proof fn lemma_keep_push(cs: Seq<Seq<u8>>, c: Seq<u8>)
    ensures
        keep(cs.push(c)) == (if is_kept(c) {
            keep(cs).push(c)
        } else {
            keep(cs)
        }),
        is_prefix(keep(cs), keep(cs.push(c))),
{
    assert(cs.push(c).drop_last() =~= cs);
    assert(cs.push(c).last() == c);
}

/// The kept finished fields of any prefix of `s` are a prefix of `comps_w(s)`: a `..` found early is in
/// `comps_w(s)`, so the kernel may refuse at once.
pub proof fn lemma_done_prefix(s: Seq<u8>, windows: bool, i: int)
    requires
        0 <= i <= s.len(),
    ensures
        is_prefix(keep(done(s, windows, i)), comps_w(s, windows)),
    decreases s.len() - i,
{
    if i == s.len() {
        assert(s.subrange(0, i) =~= s);
        lemma_split_nonempty(s, windows);
        let p = split_w(s, windows);
        assert(p =~= p.drop_last().push(p.last()));
        lemma_keep_push(p.drop_last(), p.last());
    } else {
        lemma_done_prefix(s, windows, i + 1);
        lemma_split_step(s, windows, i);
        if sep(s[i], windows) {
            let p = split_w(s.subrange(0, i), windows);
            assert(p =~= p.drop_last().push(p.last()));
            lemma_keep_push(p.drop_last(), p.last());
        }
    }
}

/// One step of lexical normalisation over components (`posixpath.normpath`, and `ntpath.normpath` after the
/// drive): ε and `.` vanish, `..` drops the last normal component (and is kept when there is none to drop, as in
/// a relative path), anything else is appended.
pub open spec fn norm_step(stack: Seq<Seq<u8>>, c: Seq<u8>) -> Seq<Seq<u8>> {
    if !is_kept(c) {
        stack
    } else if is_dotdot(c) {
        if stack.len() > 0 && !is_dotdot(stack.last()) {
            stack.drop_last()
        } else {
            stack.push(c)
        }
    } else {
        stack.push(c)
    }
}

/// Normalising `cs` on top of the already normalised `stack`.
pub open spec fn norm_from(stack: Seq<Seq<u8>>, cs: Seq<Seq<u8>>) -> Seq<Seq<u8>>
    decreases cs.len(),
{
    if cs.len() == 0 {
        stack
    } else {
        norm_step(norm_from(stack, cs.drop_last()), cs.last())
    }
}

/// `normpath` over the components of a path.
pub open spec fn normpath(cs: Seq<Seq<u8>>) -> Seq<Seq<u8>> {
    norm_from(Seq::<Seq<u8>>::empty(), cs)
}

/// Normalisation is a left fold: `norm_from(st, a ++ b) = norm_from(norm_from(st, a), b)`.
pub proof fn lemma_norm_from_append(st: Seq<Seq<u8>>, a: Seq<Seq<u8>>, b: Seq<Seq<u8>>)
    ensures
        norm_from(st, a + b) == norm_from(norm_from(st, a), b),
    decreases b.len(),
{
    if b.len() == 0 {
        assert(a + b =~= a);
    } else {
        lemma_norm_from_append(st, a, b.drop_last());
        assert((a + b).drop_last() =~= a + b.drop_last());
        assert((a + b).last() == b.last());
    }
}

/// Plain components are appended unchanged.
pub proof fn lemma_norm_from_plain(st: Seq<Seq<u8>>, v: Seq<Seq<u8>>, windows: bool)
    requires
        forall|k: int| 0 <= k < v.len() ==> plain_comp(#[trigger] v[k], windows),
    ensures
        norm_from(st, v) == st + v,
    decreases v.len(),
{
    if v.len() == 0 {
        assert(st + v =~= st);
    } else {
        let w = v.drop_last();
        assert forall|k: int| 0 <= k < w.len() implies plain_comp(#[trigger] w[k], windows) by {
            assert(w[k] == v[k]);
        }
        lemma_norm_from_plain(st, w, windows);
        assert(plain_comp(v[v.len() - 1], windows));
        assert((st + w).push(v.last()) =~= st + v);
    }
}

/// P5: `∀D. normpath(D) = D ⇒ normpath(D ++ v) = D ++ v` for plain components `v`, and `D` is a prefix of the
/// result. With `D` the destination and `v` the components of an accepted name (`spans_ok` makes every one plain),
/// normalising leaves the joined path as it is. This does not check how the model treats `..` (plain components
/// exclude it); containment itself rests on P1/P3.
pub proof fn lemma_normpath_append_plain(d: Seq<Seq<u8>>, v: Seq<Seq<u8>>, windows: bool)
    requires
        normpath(d) == d,
        forall|k: int| 0 <= k < v.len() ==> plain_comp(#[trigger] v[k], windows),
    ensures
        normpath(d + v) == d + v,
        (d + v).subrange(0, d.len() as int) == d,
{
    lemma_norm_from_append(Seq::<Seq<u8>>::empty(), d, v);
    lemma_norm_from_plain(d, v, windows);
    assert((d + v).subrange(0, d.len() as int) =~= d);
}

/// Why `..` is refused: after a normalised directory `D` whose last component is not `..`, a `..` component drops
/// that component, so the result is shorter than `D` and no longer lies below it.
pub proof fn lemma_normpath_dotdot(d: Seq<Seq<u8>>, c: Seq<u8>)
    requires
        normpath(d) == d,
        d.len() > 0,
        !is_dotdot(d.last()),
        is_dotdot(c),
    ensures
        normpath(d.push(c)) == d.drop_last(),
        normpath(d.push(c)).len() < d.len(),
{
    let one = seq![c];
    lemma_norm_from_append(Seq::<Seq<u8>>::empty(), d, one);
    assert(d + one =~= d.push(c));
    assert(one.drop_last() =~= Seq::<Seq<u8>>::empty());
    assert(one.last() == c);
    assert(norm_from(d, Seq::<Seq<u8>>::empty()) == d);
    assert(norm_from(d, one) == norm_step(norm_from(d, one.drop_last()), one.last()));
    assert(is_kept(c) && is_dotdot(c));
}

} // verus!
