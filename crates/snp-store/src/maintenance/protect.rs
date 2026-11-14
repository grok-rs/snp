//! Verified kernel (ADR-095): the gc patch protection of `patches/` (D-DATA-04, ARCHITECTURE §5.6), called by
//! `super` (`sweep` step 4, `clean`, `protection`, `journal_patch`) and `crate::marker::path_from_json`.
//!
//! Spec and lemmas: `proofs/verus/spec/gc_protect.rs`. Guarantees and non-guarantees: docs/VERIFICATION.md §9.2
//! (K8). Plain Rust: the contract lives in `verus_keep_ghost` attributes and imports, which only Verus sets.
//!
//! What is decided here: which entry names a patch protects ([`is_patch_or_sibling`], [`Protection::covers`]),
//! what one decoded stash journal protects ([`journal_verdict`], with the byte-string codec's [`unhex`]), how the
//! journals of `patches/` fold into one [`Protection`] ([`add_journal`]), and which entries `sweep` and `clean` may
//! remove ([`sweep_deletable`], [`clean_candidate`]). The shell keeps the I/O: the directory listing, the reads,
//! the `(len, mtime)` cache, `serde_json`, `Path::file_name` and the removals.
#[cfg(verus_keep_ghost)]
use crate::spec::gc_protect::{
    clean_spec, covered, covers_spec, done_word, failed_word, hex_digit_val, lemma_unhex_odd,
    lemma_unhex_step, opt_cat, patch_word, prot_view, restored_state, restored_word,
    rolling_back_word, sibling, staging_fixed_word, stashed_word, step, sweep_spec, unhex_spec,
    unrestored_state, verdict_spec, verdict_view, views,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// What the stash journals of `patches/` protect from removal (D-DATA-04, ARCHITECTURE §5.6).
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
pub(crate) enum Protection<W> {
    /// The file names of the patches of unrestored journals; each protects itself and its `<name>.*` siblings.
    Patches(Vec<Vec<u8>>),
    /// A journal (or the directory listing, then `patches/` itself) could not be read or understood: what it
    /// protects is unknown, so every patch is kept (fail closed). `W` says which, for the report.
    All(W),
}

impl<W> Protection<W> {
    /// Whether the entry `name` of `patches/` is protected: always under [`Protection::All`], else when it is one of
    /// the patches or a `<patch>.*` sibling of one (G1).
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            r == covers_spec(prot_view(*self), name@),
    ))]
    pub(crate) fn covers(&self, name: &[u8]) -> bool {
        match self {
            Self::All(_) => true,
            Self::Patches(names) => covered_by(names.as_slice(), name),
        }
    }
}

/// Whether some patch of `names` protects `name`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == covered(views(names@), name@),
))]
fn covered_by(names: &[Vec<u8>], name: &[u8]) -> bool {
    let mut k: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            k <= names.len(),
            forall|j: int| 0 <= j < k ==> !sibling(name@, #[trigger] views(names@)[j]),
        decreases names.len() - k,
    ))]
    while k < names.len() {
        match names.get(k) {
            Some(p) => {
                if is_patch_or_sibling(name, p.as_slice()) {
                    #[cfg(verus_keep_ghost)]
                    proof! {
                        assert(sibling(name@, views(names@)[k as int]));
                    }
                    return true;
                }
            }
            None => return false,
        }
        k = k.saturating_add(1);
    }
    false
}

/// Whether `name` is the patch `patch` or one of its `<patch>.*` siblings (the `<patch>.now` backup of the K+ hint,
/// the kept `.ita` and `.paths` lists, their temporary files). `patch<s>-<pid>-<n>` is another patch, not a sibling.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == sibling(name@, patch@),
))]
pub(crate) fn is_patch_or_sibling(name: &[u8], patch: &[u8]) -> bool {
    if !is_prefix(patch, name) {
        return false;
    }
    match name.get(patch.len()) {
        None => true,
        Some(&c) => c == b'.',
    }
}

/// Whether `s` starts with `pre`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == (pre.len() <= s.len() && s@.subrange(0, pre.len() as int) == pre@),
))]
fn is_prefix(pre: &[u8], s: &[u8]) -> bool {
    if s.len() < pre.len() {
        return false;
    }
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            i <= pre.len() <= s.len(),
            forall|j: int| 0 <= j < i ==> s@[j] == pre@[j],
        decreases pre.len() - i,
    ))]
    while i < pre.len() {
        match (s.get(i), pre.get(i)) {
            (Some(a), Some(b)) => {
                if *a != *b {
                    #[cfg(verus_keep_ghost)]
                    proof! {
                        assert(s@.subrange(0, pre.len() as int)[i as int] != pre@[i as int]);
                    }
                    return false;
                }
            }
            _ => return false,
        }
        i = i.saturating_add(1);
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(s@.subrange(0, pre.len() as int) =~= pre@);
    }
    true
}

/// Whether `a` and `b` hold the same bytes.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == (a@ == b@),
))]
fn same(a: &[u8], b: &[u8]) -> bool {
    let r = a.len() == b.len() && is_prefix(b, a);
    #[cfg(verus_keep_ghost)]
    proof! {
        if a.len() == b.len() {
            assert(a@.subrange(0, b.len() as int) =~= a@);
        }
    }
    r
}

/// Whether an entry of `patches/` may be removed by `sweep`: a file (not a directory) named `patch…`, at least
/// `min_age` seconds old (`None`: the age is unknown), and not protected (G2). Nothing is removable under
/// [`Protection::All`].
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == sweep_spec(prot_view(*p), name@, is_dir, age, min_age),
))]
#[expect(
    clippy::byte_char_slices,
    reason = "verus: the words are proved equal to the spec's; Verus does not see the bytes of a byte string"
)]
pub(crate) fn sweep_deletable<W>(
    p: &Protection<W>,
    name: &[u8],
    is_dir: bool,
    age: Option<u64>,
    min_age: u64,
) -> bool {
    let word: [u8; 5] = [b'p', b'a', b't', b'c', b'h'];
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(word@ =~= patch_word());
    }
    !is_dir && is_prefix(word.as_slice(), name) && old_enough(age, min_age) && !p.covers(name)
}

/// Whether `clean` considers removing an entry of `patches/`: not a journal and at least `min_age` seconds old
/// (`None`: the age is unknown). `clean` then removes it unless the journals, re-read, protect it (G2).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == clean_spec(journal, age, min_age),
))]
pub(crate) fn clean_candidate(journal: bool, age: Option<u64>, min_age: u64) -> bool {
    !journal && old_enough(age, min_age)
}

/// `age >= min_age`, false when the age is unknown.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == (match age {
            Some(a) => a >= min_age,
            None => false,
        }),
))]
fn old_enough(age: Option<u64>, min_age: u64) -> bool {
    match age {
        Some(a) => a >= min_age,
        None => false,
    }
}

/// A journal that is not understood (or, in the fold, not readable).
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
pub(crate) struct NotUnderstood;

/// The JSON shape of a journal's `patch` field, as the shell's decoder found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, verifier::external_derive)]
pub(crate) enum PatchKind {
    /// `null`, or no `patch` field.
    Null,
    /// A JSON string; the payload is its UTF-8 bytes.
    Str,
    /// An object with a string field `hex`; the payload is that string's bytes.
    Hex,
    /// Anything else.
    Other,
}

/// The path bytes of the patch a decoded journal protects (G4): `Ok(None)` when it protects none (state `restored`
/// or `done`, or no patch journaled yet: `patch` null in a state other than an unrestored one), `Err` when the
/// journal is not understood (an unrestored state — `stashed`, `staging-fixed`, `rolling-back`, `failed`, snp-stash
/// `is_unrestored` — without a patch, or a `patch` that does not decode). `state` is the journal's `state` string
/// and `payload` the bytes [`PatchKind`] names; a `{"hex": …}` path is decoded with [`unhex`].
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        verdict_view(r) == verdict_spec(state@, kind, payload@),
))]
pub(crate) fn journal_verdict(
    state: &[u8],
    kind: PatchKind,
    payload: &[u8],
) -> Result<Option<Vec<u8>>, NotUnderstood> {
    if is_restored(state) {
        return Ok(None);
    }
    match kind {
        PatchKind::Null => {
            if is_unrestored(state) {
                Err(NotUnderstood)
            } else {
                Ok(None)
            }
        }
        PatchKind::Str => {
            let mut v = Vec::new();
            v.extend_from_slice(payload);
            Ok(Some(v))
        }
        PatchKind::Hex => match unhex(payload) {
            Some(b) => Ok(Some(b)),
            None => Err(NotUnderstood),
        },
        PatchKind::Other => Err(NotUnderstood),
    }
}

/// `restored` or `done`: the patch's content is back in the worktree.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == restored_state(state@),
))]
#[expect(
    clippy::byte_char_slices,
    reason = "verus: the words are proved equal to the spec's; Verus does not see the bytes of a byte string"
)]
fn is_restored(state: &[u8]) -> bool {
    let restored: [u8; 8] = [b'r', b'e', b's', b't', b'o', b'r', b'e', b'd'];
    let done: [u8; 4] = [b'd', b'o', b'n', b'e'];
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(restored@ =~= restored_word());
        assert(done@ =~= done_word());
    }
    same(state, restored.as_slice()) || same(state, done.as_slice())
}

/// `stashed`, `staging-fixed`, `rolling-back` or `failed` (snp-stash `is_unrestored`): the run never restored.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == unrestored_state(state@),
))]
#[expect(
    clippy::byte_char_slices,
    reason = "verus: the words are proved equal to the spec's; Verus does not see the bytes of a byte string"
)]
fn is_unrestored(state: &[u8]) -> bool {
    let stashed: [u8; 7] = [b's', b't', b'a', b's', b'h', b'e', b'd'];
    let staging_fixed: [u8; 13] = [
        b's', b't', b'a', b'g', b'i', b'n', b'g', b'-', b'f', b'i', b'x', b'e', b'd',
    ];
    let rolling_back: [u8; 12] = [
        b'r', b'o', b'l', b'l', b'i', b'n', b'g', b'-', b'b', b'a', b'c', b'k',
    ];
    let failed: [u8; 6] = [b'f', b'a', b'i', b'l', b'e', b'd'];
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(stashed@ =~= stashed_word());
        assert(staging_fixed@ =~= staging_fixed_word());
        assert(rolling_back@ =~= rolling_back_word());
        assert(failed@ =~= failed_word());
    }
    same(state, stashed.as_slice())
        || same(state, staging_fixed.as_slice())
        || same(state, rolling_back.as_slice())
        || same(state, failed.as_slice())
}

/// Decodes lower-case or upper-case hex (the byte-string codec of `marker::path_to_json` and snp-stash
/// `enc_bytes`): `None` for an odd length or a byte that is not a hex digit. Proved: `unhex(hex(b)) = Some(b)` for
/// every `b`, in either case (`lemma_unhex_hex`).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        match r {
            Some(v) => unhex_spec(digits@) == Some(v@),
            None => unhex_spec(digits@) is None,
        },
))]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "verus: two hex digits make at most 255, and the index stays below the even length"
)]
pub(crate) fn unhex(digits: &[u8]) -> Option<Vec<u8>> {
    let len = digits.len();
    if !len.is_multiple_of(2) {
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_unhex_odd(digits@);
        }
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut at: usize = 0;
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(digits@.subrange(0, len as int) =~= digits@);
        assert(out@ + digits@ =~= digits@);
    }
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            at <= len == digits.len(),
            len % 2 == 0,
            at % 2 == 0,
            unhex_spec(digits@) == opt_cat(out@, unhex_spec(digits@.subrange(at as int, len as int))),
        decreases len - at,
    ))]
    while at < len {
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_unhex_step(digits@, at as int);
        }
        let pair = match (digits.get(at), digits.get(at + 1)) {
            (Some(&hi), Some(&lo)) => (hex_val(hi), hex_val(lo)),
            _ => return None,
        };
        match pair {
            (Some(hi), Some(lo)) => {
                #[cfg(verus_keep_ghost)]
                proof_decl! {
                    let ghost before = out@;
                }
                out.push(hi * 16 + lo);
                #[cfg(verus_keep_ghost)]
                proof! {
                    assert(out@ =~= before + seq![(hi * 16 + lo) as u8]);
                }
            }
            _ => return None,
        }
        at += 2;
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(digits@.subrange(len as int, len as int) =~= Seq::<u8>::empty());
        assert(out@ + Seq::<u8>::empty() =~= out@);
    }
    Some(out)
}

/// The value of a hex digit (either case).
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == hex_digit_val(c),
))]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "verus: each subtraction stays within its digit range"
)]
fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Folds one journal into the patches protected so far (G3, G4): `false` when the journal is not understood (the
/// whole protection becomes [`Protection::All`]); otherwise its patch, if any, is added to `names`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        (if r {
            Some(views(final(names)@))
        } else {
            None
        }) == step(Some(views(old(names)@)), verdict_view(verdict)),
))]
pub(crate) fn add_journal(
    names: &mut Vec<Vec<u8>>,
    verdict: Result<Option<Vec<u8>>, NotUnderstood>,
) -> bool {
    match verdict {
        Ok(Some(name)) => {
            #[cfg(verus_keep_ghost)]
            proof_decl! {
                let ghost before = names@;
                let ghost n = name@;
            }
            names.push(name);
            #[cfg(verus_keep_ghost)]
            proof! {
                assert(views(names@) =~= views(before).push(n));
            }
            true
        }
        Ok(None) => true,
        Err(NotUnderstood) => false,
    }
}
