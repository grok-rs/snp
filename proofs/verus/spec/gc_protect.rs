//! Spec of kernel K8, the gc patch protection of `patches/` (docs/VERIFICATION.md §9.2, ADR-095; D-DATA-04,
//! ARCHITECTURE §5.6).
//!
//! The store keeps the stash's recovery data: every journal, the patch of every unrestored journal and that patch's
//! `<patch>.*` siblings. `sweep` removes a `patch…` file at least 30 days old that no journal protects; `clean`
//! removes every other entry at least an hour old that the journals, re-read, do not protect. A journal that
//! cannot be read or understood protects everything (fail closed).
//!
//! Journals are modelled after decoding: `verdict_spec` is what one journal protects (`None`: not understood;
//! `Some(None)`: nothing; `Some(Some(p))`: the patch at path bytes `p`), and `fold` is the shell's loop over the
//! journals of `patches/` (`protection_capped`): `None` is `Protection::All`. Properties (lemmas):
//!
//! - G1 `lemma_sibling_iff`, `lemma_other_patch`, `lemma_all_covers`: a patch protects exactly its own name and
//!   `name ++ "." ++ s`; `patch<s>-<pid>-<n>` is not protected by `patch<s>-<pid>`; `All` protects every name.
//! - G2 `lemma_sweep_deletable`, `lemma_clean_candidate`, `lemma_clean_step`, `lemma_sweep_vs_journals`,
//!   `lemma_clean_vs_journals`: a removable entry is old enough and not protected, every journal was understood and
//!   no journal's patch protects it; for sweep it is also a file named `patch…`, for clean not a journal.
//! - G3 `lemma_fold_monotone`: adding journals (any number, anywhere in the listing order) never unprotects a name.
//! - G4 `lemma_fold_covers`, `lemma_verdict`, `lemma_unhex_hex`: the fold is `All` exactly when some journal is not
//!   understood, otherwise it protects exactly the patches the journals name; an unrestored journal without a patch
//!   is not understood, a restored one protects nothing, and a `{"hex": …}` path decodes to its bytes
//!   (`unhex(hex(b)) = Some(b)`, either case).
use crate::snp_store_maintenance_protect::{CleanStep, NotUnderstood, PatchKind, Protection};
use vstd::prelude::*;

verus! {

// ---------------------------------------------------------------- G1: which names a patch protects
/// `name` is `patch` or starts with `patch ++ "."`.
pub open spec fn sibling(name: Seq<u8>, patch: Seq<u8>) -> bool {
    &&& patch.len() <= name.len()
    &&& name.subrange(0, patch.len() as int) == patch
    &&& (name.len() == patch.len() || name[patch.len() as int] == 46u8)
}

/// The byte sequences of a list of byte vectors.
pub open spec fn views(v: Seq<Vec<u8>>) -> Seq<Seq<u8>> {
    v.map_values(|x: Vec<u8>| x@)
}

/// Some patch of `names` protects `name`.
pub open spec fn covered(names: Seq<Seq<u8>>, name: Seq<u8>) -> bool {
    exists|k: int| 0 <= k < names.len() && sibling(name, #[trigger] names[k])
}

/// A protection as the spec sees it: `None` is `All`, `Some(names)` the patches.
pub(crate) open spec fn prot_view<W>(p: Protection<W>) -> Option<Seq<Seq<u8>>> {
    match p {
        Protection::All(_) => None,
        Protection::Patches(v) => Some(views(v@)),
    }
}

/// Whether a protection protects `name`.
pub open spec fn covers_spec(p: Option<Seq<Seq<u8>>>, name: Seq<u8>) -> bool {
    match p {
        None => true,
        Some(names) => covered(names, name),
    }
}

/// G1: a patch protects exactly itself and `patch ++ "." ++ s` for every `s`.
pub proof fn lemma_sibling_iff(name: Seq<u8>, patch: Seq<u8>)
    ensures
        sibling(name, patch) <==> (name == patch || exists|s: Seq<u8>|
            name == #[trigger] (patch + seq![46u8] + s)),
{
    if sibling(name, patch) {
        if name.len() == patch.len() {
            assert(name =~= name.subrange(0, patch.len() as int));
        } else {
            let s = name.subrange(patch.len() as int + 1, name.len() as int);
            assert(name =~= patch + seq![46u8] + s);
        }
    }
    if name == patch {
        assert(name.subrange(0, patch.len() as int) =~= patch);
    }
    if exists|s: Seq<u8>| name == #[trigger] (patch + seq![46u8] + s) {
        let s = choose|s: Seq<u8>| name == #[trigger] (patch + seq![46u8] + s);
        assert(name.subrange(0, patch.len() as int) =~= patch);
        assert(name[patch.len() as int] == 46u8);
    }
}

/// G1: a name that continues `patch` with a byte other than `.` (as `patch<s>-<pid>-<n>` continues
/// `patch<s>-<pid>` with `-`) is not protected by `patch`.
pub proof fn lemma_other_patch(patch: Seq<u8>, c: u8, t: Seq<u8>)
    requires
        c != 46u8,
    ensures
        !sibling(patch + seq![c] + t, patch),
{
    let name = patch + seq![c] + t;
    assert(name[patch.len() as int] == c);
}

/// G1: `All` protects every name.
pub proof fn lemma_all_covers(name: Seq<u8>)
    ensures
        covers_spec(None, name),
{
}

// ---------------------------------------------------------------- G2: what sweep and clean may remove
/// `patch`
pub open spec fn patch_word() -> Seq<u8> {
    seq![112u8, 97u8, 116u8, 99u8, 104u8]
}

/// `s` starts with `pre`.
pub open spec fn starts_with(s: Seq<u8>, pre: Seq<u8>) -> bool {
    pre.len() <= s.len() && s.subrange(0, pre.len() as int) == pre
}

/// The age is known and at least `min_age`.
pub open spec fn old_enough(age: Option<u64>, min_age: u64) -> bool {
    match age {
        Some(a) => a >= min_age,
        None => false,
    }
}

/// `sweep_deletable`: a file named `patch…`, old enough, not protected.
pub open spec fn sweep_spec(
    p: Option<Seq<Seq<u8>>>,
    name: Seq<u8>,
    is_dir: bool,
    age: Option<u64>,
    min_age: u64,
) -> bool {
    !is_dir && starts_with(name, patch_word()) && old_enough(age, min_age) && !covers_spec(p, name)
}

/// `clean_candidate`: not a journal, old enough.
pub open spec fn clean_spec(journal: bool, age: Option<u64>, min_age: u64) -> bool {
    !journal && old_enough(age, min_age)
}

/// G2 (sweep): a removable entry is a file named `patch…`, its age is known and at least the threshold, the
/// journals were all understood and none protects it.
pub proof fn lemma_sweep_deletable(
    p: Option<Seq<Seq<u8>>>,
    name: Seq<u8>,
    is_dir: bool,
    age: Option<u64>,
    min_age: u64,
)
    requires
        sweep_spec(p, name, is_dir, age, min_age),
    ensures
        !is_dir,
        starts_with(name, patch_word()),
        age is Some && age->0 >= min_age,
        p is Some && !covered(p->0, name),
{
}

/// G2 (clean's pre-check): an entry `clean` considers is not a journal and its age is known and at least the
/// threshold; `clean_step` (`lemma_clean_step`) then decides on the re-read protection.
pub proof fn lemma_clean_candidate(journal: bool, age: Option<u64>, min_age: u64)
    requires
        clean_spec(journal, age, min_age),
    ensures
        !journal,
        age is Some && age->0 >= min_age,
{
}

/// What `clean` does with an entry, as the spec sees it (`CleanStep` without the journal it reports).
pub enum CleanAct {
    Keep,
    Remove,
    Stop,
}

/// `clean_step`: an entry `clean_candidate` rejects is kept; otherwise `All` stops, a protected entry is kept and
/// an unprotected one removed.
pub open spec fn clean_act(
    p: Option<Seq<Seq<u8>>>,
    name: Seq<u8>,
    journal: bool,
    age: Option<u64>,
    min_age: u64,
) -> CleanAct {
    if !clean_spec(journal, age, min_age) {
        CleanAct::Keep
    } else {
        match p {
            None => CleanAct::Stop,
            Some(names) => if covered(names, name) {
                CleanAct::Keep
            } else {
                CleanAct::Remove
            },
        }
    }
}

/// A `CleanStep` as the spec sees it.
pub(crate) open spec fn clean_view<W>(r: CleanStep<W>) -> CleanAct {
    match r {
        CleanStep::Keep => CleanAct::Keep,
        CleanStep::Remove => CleanAct::Remove,
        CleanStep::Stop(_) => CleanAct::Stop,
    }
}

/// G2 (clean): a removed entry is not a journal, its age is known and at least the threshold, the journals (re-read)
/// were all understood and none protects it; `clean` stops exactly at a candidate under `All`.
pub proof fn lemma_clean_step(
    p: Option<Seq<Seq<u8>>>,
    name: Seq<u8>,
    journal: bool,
    age: Option<u64>,
    min_age: u64,
)
    ensures
        clean_act(p, name, journal, age, min_age) is Remove ==> {
            &&& !journal
            &&& age is Some && age->0 >= min_age
            &&& p is Some && !covered(p->0, name)
        },
        clean_act(p, name, journal, age, min_age) is Stop <==> (clean_spec(journal, age, min_age)
            && p is None),
{
}

/// G2 over the journals (sweep): an entry `sweep` removes under the protection of the journals `js` is a file named
/// `patch…`, old enough, every journal was understood and no journal's patch protects it.
pub proof fn lemma_sweep_vs_journals(
    js: Seq<Option<Option<Seq<u8>>>>,
    name: Seq<u8>,
    is_dir: bool,
    age: Option<u64>,
    min_age: u64,
)
    requires
        sweep_spec(fold(js), name, is_dir, age, min_age),
    ensures
        !is_dir,
        starts_with(name, patch_word()),
        age is Some && age->0 >= min_age,
        !some_unknown(js),
        !some_protects(js, name),
{
    lemma_fold_covers(js, name);
}

/// G2 over the journals (clean): an entry `clean` removes under the protection of the journals `js`, re-read, is
/// not a journal, old enough, every journal was understood and no journal's patch protects it.
pub proof fn lemma_clean_vs_journals(
    js: Seq<Option<Option<Seq<u8>>>>,
    name: Seq<u8>,
    journal: bool,
    age: Option<u64>,
    min_age: u64,
)
    requires
        clean_act(fold(js), name, journal, age, min_age) is Remove,
    ensures
        !journal,
        age is Some && age->0 >= min_age,
        !some_unknown(js),
        !some_protects(js, name),
{
    lemma_fold_covers(js, name);
}

// ---------------------------------------------------------------- G4: what one decoded journal protects
/// `restored`
pub open spec fn restored_word() -> Seq<u8> {
    seq![114u8, 101u8, 115u8, 116u8, 111u8, 114u8, 101u8, 100u8]
}

/// `done`
pub open spec fn done_word() -> Seq<u8> {
    seq![100u8, 111u8, 110u8, 101u8]
}

/// `stashed`
pub open spec fn stashed_word() -> Seq<u8> {
    seq![115u8, 116u8, 97u8, 115u8, 104u8, 101u8, 100u8]
}

/// `staging-fixed`
pub open spec fn staging_fixed_word() -> Seq<u8> {
    seq![115u8, 116u8, 97u8, 103u8, 105u8, 110u8, 103u8, 45u8, 102u8, 105u8, 120u8, 101u8, 100u8]
}

/// `rolling-back`
pub open spec fn rolling_back_word() -> Seq<u8> {
    seq![114u8, 111u8, 108u8, 108u8, 105u8, 110u8, 103u8, 45u8, 98u8, 97u8, 99u8, 107u8]
}

/// `failed`
pub open spec fn failed_word() -> Seq<u8> {
    seq![102u8, 97u8, 105u8, 108u8, 101u8, 100u8]
}

/// The patch's content is back in the worktree.
pub open spec fn restored_state(s: Seq<u8>) -> bool {
    s == restored_word() || s == done_word()
}

/// The run never restored (snp-stash `is_unrestored`).
pub open spec fn unrestored_state(s: Seq<u8>) -> bool {
    s == stashed_word() || s == staging_fixed_word() || s == rolling_back_word() || s
        == failed_word()
}

/// The value of a hex digit, either case.
pub open spec fn hex_digit_val(c: u8) -> Option<u8> {
    if 48 <= c <= 57 {
        Some((c - 48) as u8)
    } else if 97 <= c <= 102 {
        Some((c - 87) as u8)
    } else if 65 <= c <= 70 {
        Some((c - 55) as u8)
    } else {
        None
    }
}

/// Hex decoding, two digits per byte from the left.
pub open spec fn unhex_spec(s: Seq<u8>) -> Option<Seq<u8>>
    decreases s.len(),
{
    if s.len() == 0 {
        Some(Seq::empty())
    } else if s.len() == 1 {
        None
    } else {
        match (
            hex_digit_val(s[0]),
            hex_digit_val(s[1]),
            unhex_spec(s.subrange(2, s.len() as int)),
        ) {
            (Some(h), Some(l), Some(r)) => Some(seq![(h * 16 + l) as u8] + r),
            _ => None,
        }
    }
}

/// `Some(a ++ r)` for `Some(r)`.
pub open spec fn opt_cat(a: Seq<u8>, o: Option<Seq<u8>>) -> Option<Seq<u8>> {
    match o {
        Some(r) => Some(a + r),
        None => None,
    }
}

/// An odd number of digits does not decode.
pub proof fn lemma_unhex_odd(s: Seq<u8>)
    requires
        s.len() % 2 == 1,
    ensures
        unhex_spec(s) is None,
    decreases s.len(),
{
    if s.len() > 1 {
        lemma_unhex_odd(s.subrange(2, s.len() as int));
    }
}

/// One step of the decoding loop at an even index `i`.
pub proof fn lemma_unhex_step(s: Seq<u8>, i: int)
    requires
        0 <= i,
        i + 2 <= s.len(),
    ensures
        unhex_spec(s.subrange(i, s.len() as int)) == match (
            hex_digit_val(s[i]),
            hex_digit_val(s[i + 1]),
            unhex_spec(s.subrange(i + 2, s.len() as int)),
        ) {
            (Some(h), Some(l), Some(r)) => Some(seq![(h * 16 + l) as u8] + r),
            _ => None,
        },
{
    let t = s.subrange(i, s.len() as int);
    assert(t.subrange(2, t.len() as int) =~= s.subrange(i + 2, s.len() as int));
    assert(t[0] == s[i] && t[1] == s[i + 1]);
}

/// The hex digit of a nibble: `0-9a-f`, or `0-9A-F` when `upper`.
pub open spec fn hex_char(v: u8, upper: bool) -> u8 {
    if v < 10 {
        (48 + v) as u8
    } else if upper {
        (55 + v) as u8
    } else {
        (87 + v) as u8
    }
}

/// Hex encoding, two digits per byte (snp-stash `enc_bytes` and `keys::hex` write lower case).
pub open spec fn hex_spec(b: Seq<u8>, upper: bool) -> Seq<u8>
    decreases b.len(),
{
    if b.len() == 0 {
        Seq::empty()
    } else {
        seq![hex_char(b[0] / 16, upper), hex_char(b[0] % 16, upper)] + hex_spec(
            b.drop_first(),
            upper,
        )
    }
}

/// G4: the codec round trip `unhex(hex(b)) = Some(b)`, in either case.
pub proof fn lemma_unhex_hex(b: Seq<u8>, upper: bool)
    ensures
        unhex_spec(hex_spec(b, upper)) == Some(b),
    decreases b.len(),
{
    if b.len() > 0 {
        let x = b[0];
        let e = hex_spec(b, upper);
        lemma_unhex_hex(b.drop_first(), upper);
        assert(e.subrange(2, e.len() as int) =~= hex_spec(b.drop_first(), upper));
        assert(hex_digit_val(hex_char(x / 16, upper)) == Some((x / 16) as u8));
        assert(hex_digit_val(hex_char(x % 16, upper)) == Some((x % 16) as u8));
        assert((x / 16) * 16 + x % 16 == x);
        assert(seq![x] + b.drop_first() =~= b);
    }
}

/// What one decoded journal protects: `None` not understood, `Some(None)` nothing, `Some(Some(p))` the patch at
/// path bytes `p` (`journal_patch` then takes `p`'s file name).
pub(crate) open spec fn verdict_spec(state: Seq<u8>, kind: PatchKind, payload: Seq<u8>) -> Option<
    Option<Seq<u8>>,
> {
    if restored_state(state) {
        Some(None)
    } else {
        match kind {
            PatchKind::Null => if unrestored_state(state) {
                None
            } else {
                Some(None)
            },
            PatchKind::Str => Some(Some(payload)),
            PatchKind::Hex => match unhex_spec(payload) {
                Some(b) => Some(Some(b)),
                None => None,
            },
            PatchKind::Other => None,
        }
    }
}

/// A journal verdict as the spec sees it.
pub(crate) open spec fn verdict_view(r: Result<Option<Vec<u8>>, NotUnderstood>) -> Option<
    Option<Seq<u8>>,
> {
    match r {
        Err(_) => None,
        Ok(None) => Some(None),
        Ok(Some(v)) => Some(Some(v@)),
    }
}

/// G4 for one journal: a restored journal protects nothing; an unrestored one without a patch is not understood;
/// an unrestored one whose patch is the string `p` or `{"hex": hex(p)}` (either case) protects `p`; any other
/// shape of `patch` is not understood.
pub(crate) proof fn lemma_verdict(state: Seq<u8>, p: Seq<u8>, upper: bool)
    ensures
        restored_state(state) ==> forall|k: PatchKind, x: Seq<u8>| #[trigger]
            verdict_spec(state, k, x) == Some(None::<Seq<u8>>),
        unrestored_state(state) ==> verdict_spec(state, PatchKind::Null, p) is None,
        unrestored_state(state) ==> verdict_spec(state, PatchKind::Str, p) == Some(Some(p)),
        unrestored_state(state) ==> verdict_spec(state, PatchKind::Hex, hex_spec(p, upper)) == Some(
            Some(p),
        ),
        !restored_state(state) ==> verdict_spec(state, PatchKind::Other, p) is None,
{
    lemma_unhex_hex(p, upper);
    if unrestored_state(state) {
        assert(!restored_state(state));
    }
}

// ---------------------------------------------------------------- G3, G4: the fold over the journals
/// One journal folded in: `All` stays `All`, a journal not understood makes it `All`, a patch is added.
pub open spec fn step(acc: Option<Seq<Seq<u8>>>, v: Option<Option<Seq<u8>>>) -> Option<
    Seq<Seq<u8>>,
> {
    match acc {
        None => None,
        Some(names) => match v {
            None => None,
            Some(None) => Some(names),
            Some(Some(n)) => Some(names.push(n)),
        },
    }
}

/// The protection of a listing of journals, in listing order (`protection_capped`; a journal deleted meanwhile
/// is skipped, i.e. `Some(None)`).
pub open spec fn fold(js: Seq<Option<Option<Seq<u8>>>>) -> Option<Seq<Seq<u8>>>
    decreases js.len(),
{
    if js.len() == 0 {
        Some(Seq::empty())
    } else {
        step(fold(js.drop_last()), js.last())
    }
}

/// Some journal of `js` is not understood.
pub open spec fn some_unknown(js: Seq<Option<Option<Seq<u8>>>>) -> bool {
    exists|k: int| 0 <= k < js.len() && #[trigger] js[k] is None
}

/// Some journal of `js` names a patch that protects `name`.
pub open spec fn some_protects(js: Seq<Option<Option<Seq<u8>>>>, name: Seq<u8>) -> bool {
    exists|k: int|
        0 <= k < js.len() && #[trigger] js[k] is Some && js[k]->0 is Some && sibling(
            name,
            js[k]->0->0,
        )
}

/// G4: the fold is `All` exactly when some journal is not understood; otherwise its patches are exactly the ones
/// the journals name.
pub proof fn lemma_fold_names(js: Seq<Option<Option<Seq<u8>>>>)
    ensures
        fold(js) is None <==> some_unknown(js),
        fold(js) is Some ==> forall|n: Seq<u8>| #[trigger]
            fold(js)->0.contains(n) <==> exists|k: int|
                0 <= k < js.len() && #[trigger] js[k] == Some(Some(n)),
    decreases js.len(),
{
    if js.len() > 0 {
        let init = js.drop_last();
        lemma_fold_names(init);
        assert forall|k: int| 0 <= k < init.len() implies #[trigger] init[k] == js[k] by {}
        if some_unknown(init) {
            let k = choose|k: int| 0 <= k < init.len() && #[trigger] init[k] is None;
            assert(js[k] is None);
        }
        if some_unknown(js) && !some_unknown(init) {
            let k = choose|k: int| 0 <= k < js.len() && #[trigger] js[k] is None;
            if k < js.len() - 1 {
                assert(init[k] is None);
            }
        }
        if fold(js) is Some {
            let names = fold(init)->0;
            assert(fold(js)->0 == names || fold(js)->0 == names.push(js.last()->0->0));
            assert forall|n: Seq<u8>| #[trigger]
                fold(js)->0.contains(n) <==> exists|k: int|
                    0 <= k < js.len() && #[trigger] js[k] == Some(Some(n)) by {
                if fold(js)->0.contains(n) {
                    if names.contains(n) {
                        let k = choose|k: int|
                            0 <= k < init.len() && #[trigger] init[k] == Some(Some(n));
                        assert(js[k] == Some(Some(n)));
                    } else {
                        let i = choose|i: int| 0 <= i < fold(js)->0.len() && fold(js)->0[i] == n;
                        assert(i == names.len());
                        assert(js[js.len() - 1] == Some(Some(n)));
                    }
                }
                if exists|k: int| 0 <= k < js.len() && #[trigger] js[k] == Some(Some(n)) {
                    let k = choose|k: int| 0 <= k < js.len() && #[trigger] js[k] == Some(Some(n));
                    if k < js.len() - 1 {
                        assert(init[k] == Some(Some(n)));
                        assert(names.contains(n));
                        let i = choose|i: int| 0 <= i < names.len() && names[i] == n;
                        assert(fold(js)->0[i] == n);
                    } else {
                        assert(fold(js)->0 == names.push(n));
                        assert(fold(js)->0[names.len() as int] == n);
                    }
                }
            }
        }
    }
}

/// G4: a name is protected by the fold exactly when some journal is not understood or some journal's patch
/// protects it.
pub proof fn lemma_fold_covers(js: Seq<Option<Option<Seq<u8>>>>, name: Seq<u8>)
    ensures
        covers_spec(fold(js), name) <==> (some_unknown(js) || some_protects(js, name)),
{
    lemma_fold_names(js);
    if fold(js) is Some {
        let names = fold(js)->0;
        if covered(names, name) {
            let i = choose|i: int| 0 <= i < names.len() && sibling(name, #[trigger] names[i]);
            assert(names.contains(names[i]));
            let k = choose|k: int| 0 <= k < js.len() && #[trigger] js[k] == Some(Some(names[i]));
            assert(js[k] is Some && js[k]->0 is Some && sibling(name, js[k]->0->0));
        }
        if some_protects(js, name) {
            let k = choose|k: int|
                0 <= k < js.len() && #[trigger] js[k] is Some && js[k]->0 is Some && sibling(
                    name,
                    js[k]->0->0,
                );
            let n = js[k]->0->0;
            assert(js[k] == Some(Some(n)));
            assert(names.contains(n));
            let i = choose|i: int| 0 <= i < names.len() && names[i] == n;
            assert(sibling(name, names[i]));
        }
    }
}

/// G3: adding journals never unprotects a name: when every journal of `js` is also a journal of `more` (in any
/// order, with any others added), whatever `js` protects, `more` protects.
pub proof fn lemma_fold_monotone(
    js: Seq<Option<Option<Seq<u8>>>>,
    more: Seq<Option<Option<Seq<u8>>>>,
    name: Seq<u8>,
)
    requires
        forall|k: int| 0 <= k < js.len() ==> more.contains(#[trigger] js[k]),
        covers_spec(fold(js), name),
    ensures
        covers_spec(fold(more), name),
{
    lemma_fold_covers(js, name);
    lemma_fold_covers(more, name);
    if some_unknown(js) {
        let k = choose|k: int| 0 <= k < js.len() && #[trigger] js[k] is None;
        assert(more.contains(js[k]));
        let m = choose|m: int| 0 <= m < more.len() && more[m] == js[k];
        assert(more[m] is None);
    } else {
        let k = choose|k: int|
            0 <= k < js.len() && #[trigger] js[k] is Some && js[k]->0 is Some && sibling(
                name,
                js[k]->0->0,
            );
        assert(more.contains(js[k]));
        let m = choose|m: int| 0 <= m < more.len() && more[m] == js[k];
        assert(more[m] is Some && more[m]->0 is Some && sibling(name, more[m]->0->0));
    }
}

} // verus!
