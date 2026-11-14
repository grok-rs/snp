//! Guard unit tests on the fixture tree `fixtures/good` (a registry, a root, a spec, a Lean package and a demo crate
//! with one kernel, one trusted impl and one model). Each bad case edits the tree in memory and expects its guard.

use std::path::Path;

use super::*;

fn fixture() -> Tree {
    Tree::disk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tasks/proofs/fixtures/good"))
}

const KERN: &str = "crates/demo/src/kern.rs";
const LIB: &str = "crates/demo/src/lib.rs";

/// The fixture with its anchor, driver and oracle-test fingerprints filled in (computed, so editing the fixture
/// stays easy).
fn good() -> Tree {
    let mut t = fixture();
    let fp = fingerprint::of_anchor(&t, "crates/demo/src/lib.rs::split").unwrap();
    edit(&mut t, REGISTRY, "0000000000000000", &fp);
    let fp = fingerprint::of_file(&t, "proofs/lean/DemoMain.lean").unwrap();
    edit(&mut t, REGISTRY, "1111111111111111", &fp);
    let fp = fingerprint::of_file(&t, "crates/demo/tests/oracle.rs").unwrap();
    edit(&mut t, REGISTRY, "2222222222222222", &fp);
    t
}

fn edit(t: &mut Tree, rel: &str, find: &str, replace: &str) {
    let s = t.read(rel).unwrap();
    assert!(s.contains(find), "{rel} does not contain {find:?}");
    t.put(rel, &s.replacen(find, replace, 1));
}

fn problems(t: &Tree) -> Vec<Problem> {
    let reg = Registry::load(t).unwrap();
    check(t, &reg).unwrap()
}

/// The guards that fired (sorted, deduplicated).
fn fired(t: &Tree) -> Vec<&'static str> {
    let mut g: Vec<&'static str> = problems(t).iter().map(|p| p.guard).collect();
    g.sort_unstable();
    g.dedup();
    g
}

#[test]
fn the_good_fixture_passes() {
    let t = good();
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

#[test]
fn the_fixture_ships_placeholder_fingerprints_that_g10_rejects() {
    let p = problems(&fixture());
    assert_eq!(p.len(), 3, "{p:#?}");
    assert!(
        p.iter()
            .all(|p| p.guard == "G10" && p.msg.contains("model link stale")),
        "{p:#?}"
    );
}

// G1 ------------------------------------------------------------------------------------------------------------

#[test]
fn g1_unmounted_kernel_file() {
    let mut t = good();
    edit(
        &mut t,
        ROOT,
        "/// K9: demo kernel.\n#[verus_verify]\n#[path = \"../../crates/demo/src/kern.rs\"]\npub mod demo_kern;\n",
        "",
    );
    assert_eq!(fired(&t), ["G1"]);
}

#[test]
fn g1_mount_without_verus_verify() {
    let mut t = good();
    edit(&mut t, ROOT, "#[verus_verify]\n", "");
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G1" && p.msg.contains("#[verus_verify]")),
        "{p:#?}"
    );
    let mut t = good();
    edit(
        &mut t,
        ROOT,
        "#[verus_verify]\n",
        "#[verus_verify(external)]\n",
    );
    assert_eq!(fired(&t), ["G1", "G5"]);
}

#[test]
fn g1_mount_must_point_at_the_registered_file_under_crates() {
    let mut t = good();
    edit(
        &mut t,
        ROOT,
        "../../crates/demo/src/kern.rs",
        "../../xtask/kern.rs",
    );
    let p = problems(&t);
    assert!(
        p.iter().any(|p| p.msg.contains("must carry #[path")),
        "{p:#?}"
    );
    assert!(
        p.iter().any(|p| p.msg.contains("must point under crates/")),
        "{p:#?}"
    );
}

#[test]
fn g1_mount_without_registry_entry_and_stray_modules() {
    let mut t = good();
    edit(
        &mut t,
        ROOT,
        "pub mod demo_kern;\n",
        "pub mod demo_kern;\n\n#[verus_verify]\n#[path = \"../../crates/demo/src/fast.rs\"]\npub mod demo_fast;\n\npub mod extra;\n",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.msg.contains("`demo_fast` has no verified [[kernel]]")),
        "{p:#?}"
    );
    assert!(p.iter().any(|p| p.msg.contains("`mod extra`")), "{p:#?}");
}

#[test]
fn g1_marker_line_and_unregistered_marked_files() {
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "//! Verified kernel (ADR-095):",
        "//! Kernel:",
    );
    assert_eq!(fired(&t), ["G1"]);
    let mut t = good();
    t.put(
        "crates/demo/src/other.rs",
        "//! Verified kernel (ADR-095): not registered.\nfn f() {}\n",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.file == "crates/demo/src/other.rs" && p.guard == "G1"),
        "{p:#?}"
    );
}

#[test]
fn g1_registry_shape() {
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "spec = \"proofs/verus/spec/demo.rs\"",
        "spec = \"proofs/verus/spec/nope.rs\"",
    );
    edit(
        &mut t,
        REGISTRY,
        "mutants = [{ find = \"out.push(start..i);\", replace = \"out.push(start..start);\" }]",
        "mutants = []",
    );
    let p = problems(&t);
    assert!(
        p.iter().any(|p| p.msg.contains("nope.rs does not exist")),
        "{p:#?}"
    );
    assert!(
        p.iter().any(|p| p.msg.contains("at least one mutant")),
        "{p:#?}"
    );
    let mut t = good();
    t.remove(KERN);
    assert!(fired(&t).contains(&"G1"));
    let mut t = good();
    edit(&mut t, ROOT, "#![forbid(unsafe_code)]\n", "");
    assert_eq!(fired(&t), ["G1"]);
}

#[test]
fn g1_pending_kernels_are_unmounted_and_say_so() {
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "status = \"verified\"",
        "status = \"pending\"",
    );
    edit(
        &mut t,
        KERN,
        "//! Verified kernel (ADR-095): a demo",
        "//! Kernel (ADR-095), proof pending: #123. A demo",
    );
    edit(
        &mut t,
        ROOT,
        "/// K9: demo kernel.\n#[verus_verify]\n#[path = \"../../crates/demo/src/kern.rs\"]\npub mod demo_kern;\n",
        "",
    );
    edit(
        &mut t,
        KERN,
        "#[expect(clippy::arithmetic_side_effects, reason = \"verus: i < hay.len() <= usize::MAX\")]\n",
        "",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
    // still mounted
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "status = \"verified\"",
        "status = \"pending\"",
    );
    edit(
        &mut t,
        KERN,
        "//! Verified kernel (ADR-095): a demo",
        "//! Kernel (ADR-095), proof pending: #123. A demo",
    );
    let p = problems(&t);
    assert!(
        p.iter().any(|p| p.msg.contains("must not be mounted")),
        "{p:#?}"
    );
}

// G2 ------------------------------------------------------------------------------------------------------------

#[test]
fn g2_negated_and_other_cfg_predicates() {
    for (find, replace) in [
        ("fn helper(", "#[cfg(not(verus_keep_ghost))]\nfn helper("),
        (
            "fn helper(",
            "#[cfg(any(verus_keep_ghost, test))]\nfn helper(",
        ),
        ("fn helper(", "#[cfg(windows)]\nfn helper("),
        ("fn helper(", "#[cfg(test)]\nfn helper("),
        ("fn helper(", "#[cfg_attr(test, inline)]\nfn helper("),
        (
            "        let mut i = from;\n",
            "        let mut i = from;\n        let _w = cfg!(windows);\n",
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert_eq!(fired(&t), ["G2"], "{replace}");
    }
    // `cfg!` inside another macro: also a macro call (G3).
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "            i = i.saturating_add(1);\n",
        "            i = i.saturating_add(usize::from(matches!(1, 1 if cfg!(unix))));\n",
    );
    assert_eq!(fired(&t), ["G2", "G3"]);
}

// G3 ------------------------------------------------------------------------------------------------------------

#[test]
fn g3_ghost_cfg_only_on_allowed_uses_and_proof_statements() {
    for (find, replace) in [
        ("fn helper(", "#[cfg(verus_keep_ghost)]\nfn helper("),
        ("use crate::spec::demo::{", "use crate::fast::{"),
        // A glob of a spec module shadows the prelude under Verus only.
        (
            "use crate::spec::demo::{first_from, lemma_demo, spans_ok};",
            "use crate::spec::demo::*;",
        ),
        // So does a named spec item with a prelude name.
        (
            "use crate::spec::demo::{first_from, lemma_demo, spans_ok};",
            "use crate::spec::demo::{drop, first_from, lemma_demo, spans_ok};",
        ),
        (
            "use crate::spec::demo::{first_from, lemma_demo, spans_ok};",
            "use crate::spec::demo::{first_from, lemma_demo, spans_ok as core};",
        ),
        (
            "use crate::spec::demo::{first_from, ",
            "use crate::spec::demo::{self, first_from, ",
        ),
        ("use vstd::prelude::*;", "use vstd::seq::*;"),
        (
            "        #[cfg(verus_keep_ghost)]\n        proof! {",
            "        proof! {",
        ),
        (
            "impl Scalar {",
            "verus! {\n    pub open spec fn s() -> bool { true }\n}\n\nimpl Scalar {",
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert_eq!(fired(&t), ["G3"], "{replace}");
    }
}

/// Macros: the scope of a macro is the module chain, which the Verus root does not share with production, and a
/// macro body is not parsed by the guards (review findings: `when!(cfg)`, `include!`, a parent's `matches!`).
#[test]
fn g3_kernels_call_and_define_no_macro() {
    for (find, replace) in [
        (
            "use std::ops::Range;\n",
            "use std::ops::Range;\n\nmacro_rules! when {\n    ($c:ident) => {\n        $c!(not(verus_keep_ghost))\n    };\n}\n",
        ),
        (
            "            i = i.saturating_add(1);\n",
            "            if when!(cfg) && i == 3 {\n                return None;\n            }\n            i = i.saturating_add(1);\n",
        ),
        (
            "            i = i.saturating_add(1);\n",
            "            include!(\"../../../../proofs/verus/helpers/step.rs\");\n            i = i.saturating_add(1);\n",
        ),
        ("impl Scalar {", "include!(\"items.rs\");\n\nimpl Scalar {"),
        (
            "            if hay.get(i) == Some(&needle) {",
            "            if matches!(hay.get(i), Some(&b) if b == needle) {",
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert!(fired(&t).contains(&"G3"), "{replace}: {:#?}", problems(&t));
    }
}

// G4, G5 --------------------------------------------------------------------------------------------------------

#[test]
fn g4_payloads_outside_the_allowlist() {
    for (find, replace, also_g5) in [
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, verifier::external_body)]\nfn helper(",
            true,
        ),
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, verifier::external)]\nfn helper(",
            true,
        ),
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, verifier::exec_allows_no_decreases_clause)]\nfn helper(",
            true,
        ),
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]\nfn helper(",
            true,
        ),
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, verus_verify(rlimit(5)))]\nfn helper(",
            false,
        ),
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, inline)]\nfn helper(",
            false,
        ),
        (
            "fn helper(",
            "#[cfg_attr(verus_keep_ghost, verifier::opaque)]\nfn helper(",
            false,
        ),
        (
            "fn helper(",
            "#[verifier::loop_isolation(false)]\nfn helper(",
            false,
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        let want: &[&str] = if also_g5 { &["G4", "G5"] } else { &["G4"] };
        assert_eq!(fired(&t), want, "{replace}");
    }
}

#[test]
fn g4_allowlisted_payloads_pass() {
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "fn helper(",
        "#[cfg_attr(verus_keep_ghost, verifier::loop_isolation(false))]\n#[cfg_attr(verus_keep_ghost, verifier::spinoff_prover)]\n#[cfg_attr(verus_keep_ghost, verus_verify)]\nfn helper(",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

#[test]
fn g5_cheat_tokens_in_kernels_specs_and_the_root() {
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "lemma_demo();",
        "lemma_demo();\n            admit();",
    );
    assert_eq!(fired(&t), ["G5"]);
    let mut t = good();
    edit(
        &mut t,
        "proofs/verus/spec/demo.rs",
        "pub proof fn lemma_demo() {\n}",
        "pub proof fn lemma_demo() {\n    assume(false);\n}",
    );
    assert_eq!(fired(&t), ["G5"]);
    let mut t = good();
    t.put(
        "proofs/verus/spec/lib/extra.rs",
        "verus! { #[verifier::external_body] pub fn f() {} }\n",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G5" && p.file == "proofs/verus/spec/lib/extra.rs"),
        "{p:#?}"
    );
    let mut t = good();
    edit(
        &mut t,
        ROOT,
        "pub mod spec;",
        "pub mod spec;\n\nverus! { pub assume_specification[ std::mem::swap ] ; }",
    );
    assert_eq!(fired(&t), ["G5"]);
    // A raw identifier hides a cheat from a name comparison; rustc resolves it to the plain name.
    let mut t = good();
    edit(
        &mut t,
        "proofs/verus/spec/demo.rs",
        "/// A lemma the kernel calls.",
        "#[verifier::r#exec_allows_no_decreases_clause]\npub fn drop<T>(_x: T)\n    ensures\n        false,\n{\n    loop {\n    }\n}\n\n/// A lemma the kernel calls.",
    );
    let p = problems(&t);
    for want in [
        "cheat token `exec_allows_no_decreases_clause`",
        "raw identifier",
        "an exec `fn`",
    ] {
        assert!(
            p.iter().any(|p| p.guard == "G5" && p.msg.contains(want)),
            "{want}: {p:#?}"
        );
    }
    let mut t = good();
    edit(&mut t, KERN, "let mut i = from;", "let mut r#i = from;");
    assert_eq!(fired(&t), ["G5"]);
    // Spec files hold spec/proof items only: no trait, impl or macro definition, no include!, no #[path].
    for extra in [
        "pub trait T { fn f(&self); }",
        "impl Foo { }",
        "macro_rules! m { () => {} }",
        "include!(\"other.rs\");",
        "#[path = \"../../../crates/demo/src/x.rs\"]\nmod x;",
    ] {
        let mut t = good();
        edit(
            &mut t,
            "proofs/verus/spec/demo.rs",
            "/// A lemma the kernel calls.",
            &format!("{extra}\n\n/// A lemma the kernel calls."),
        );
        assert_eq!(fired(&t), ["G5"], "{extra}");
    }
    // Every file under proofs/verus/ is scanned, and only the root, specs and trusted items may live there.
    let mut t = good();
    t.put(
        "proofs/verus/helpers/step.rs",
        "if cfg!(not(verus_keep_ghost)) { admit(); }\n",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G1" && p.file == "proofs/verus/helpers/step.rs"),
        "{p:#?}"
    );
    assert!(
        p.iter()
            .any(|p| p.guard == "G5" && p.file == "proofs/verus/helpers/step.rs"),
        "{p:#?}"
    );
    // words in comments and strings are not tokens
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "/// The scalar reference finder.",
        "/// The scalar reference finder (no assume, no admit, never external).",
    );
    edit(
        &mut t,
        "proofs/verus/spec/demo.rs",
        "/// A lemma the kernel calls.",
        "// assume nothing here\n/// A lemma the kernel calls.",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

// G6 ------------------------------------------------------------------------------------------------------------

#[test]
fn g6_requires_on_a_non_private_function() {
    let mut t = good();
    edit(&mut t, KERN, "fn helper(", "pub(crate) fn helper(");
    assert_eq!(fired(&t), ["G6"]);
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "    ensures spans_ok(r@, hay@),\n",
        "    requires hay.len() < 100,\n    ensures spans_ok(r@, hay@),\n",
    );
    assert_eq!(fired(&t), ["G6"]);
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "    const ONE: usize = 1;\n",
        "    const ONE: usize = 1;\n\n    #[cfg_attr(verus_keep_ghost, verus_spec(r => requires x > 0))]\n    pub(crate) fn dec(x: usize) -> usize {\n        x.saturating_sub(1)\n    }\n",
    );
    assert_eq!(fired(&t), ["G6"]);
}

// G7 ------------------------------------------------------------------------------------------------------------

#[test]
fn g7_verus_reasons_only_on_functions_of_mounted_kernels() {
    let mut t = good();
    edit(
        &mut t,
        LIB,
        "pub fn split(",
        "#[expect(clippy::indexing_slicing, reason = \"verus: proved elsewhere\")]\npub fn split(",
    );
    let p = problems(&t);
    assert!(p.iter().any(|p| p.guard == "G7" && p.file == LIB), "{p:#?}");
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "pub(crate) struct Scalar;",
        "#[expect(dead_code, reason = \"verus: n/a\")]\npub(crate) struct Scalar;",
    );
    assert_eq!(fired(&t), ["G7"]);
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "#[expect(clippy::arithmetic_side_effects",
        "#[allow(clippy::arithmetic_side_effects",
    );
    assert_eq!(fired(&t), ["G7"]);
}

// G8 ------------------------------------------------------------------------------------------------------------

#[test]
fn g8_trusted_impls_of_kernel_traits() {
    let mut t = good();
    let reg = t.read(REGISTRY).unwrap();
    let cut = reg.find("[[trusted]]").unwrap();
    t.put(REGISTRY, reg.get(..cut).unwrap());
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G8" && p.file == "crates/demo/src/fast.rs"),
        "{p:#?}"
    );
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "oracle.rs::fast_matches_scalar",
        "oracle.rs::no_such_test",
    );
    assert_eq!(fired(&t), ["G8"]);
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "item = \"Fast: FindByte\"",
        "item = \"Slow: FindByte\"",
    );
    let p = problems(&t);
    assert!(p.iter().any(|p| p.msg.contains("stale entry")), "{p:#?}");
    assert!(
        p.iter().any(|p| p.msg.contains("`impl Fast: FindByte`")),
        "{p:#?}"
    );
}

// G9 ------------------------------------------------------------------------------------------------------------

#[test]
fn g9_paths_outside_the_file_and_std() {
    for (find, replace) in [
        (
            "x.saturating_add(Scalar::ONE)",
            "x.saturating_add(crate::kern::Scalar::ONE)",
        ),
        (
            "x.saturating_add(Scalar::ONE)",
            "x.saturating_add(super::ONE)",
        ),
        (
            "x.saturating_add(Scalar::ONE)",
            "x.saturating_add(self::super::ONE)",
        ),
        (
            "x.saturating_add(Scalar::ONE)",
            "x.saturating_add(memchr::ONE)",
        ),
        (
            "x.saturating_add(Scalar::ONE)",
            "x.saturating_add(::memchr::ONE)",
        ),
        (
            "use std::ops::Range;",
            "use std::ops::Range;\nuse crate::fast::Fast;",
        ),
        (
            "use std::ops::Range;",
            "use std::ops::Range;\nuse super::fast;",
        ),
        (
            "use std::ops::Range;",
            "use std::ops::Range;\nuse memchr::memchr;",
        ),
        ("use std::ops::Range;", "use std::ops::Range;\nuse memchr;"),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert_eq!(fired(&t), ["G9"], "{replace}");
    }
    // A crate path inside a macro (the macro call itself is G3).
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "                return out;\n",
        "                let _ = matches!(sep, crate::SEP);\n                return out;\n",
    );
    assert_eq!(fired(&t), ["G3", "G9"]);
}

#[test]
fn g9_out_of_line_modules_and_path_attributes() {
    for (find, replace, guards) in [
        (
            "impl Scalar {",
            "#[cfg_attr(verus_keep_ghost, verus_verify)]\n#[path = \"../../../proofs/verus/helpers/bail.rs\"]\nmod bail;\n\nimpl Scalar {",
            &["G4", "G9"][..],
        ),
        ("impl Scalar {", "mod h;\n\nimpl Scalar {", &["G9"][..]),
        (
            "impl Scalar {",
            "#[path = \"h.rs\"]\nmod h {\n    pub(crate) fn f() {}\n}\n\nimpl Scalar {",
            &["G9"][..],
        ),
        (
            "impl Scalar {",
            "#[cfg_attr(verus_keep_ghost, verus_verify)]\nmod h {\n    pub(crate) fn f() {}\n}\n\nimpl Scalar {",
            &["G4"][..],
        ),
        (
            "impl Scalar {",
            "extern crate alloc;\n\nimpl Scalar {",
            &["G9"][..],
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert_eq!(fired(&t), guards, "{replace}: {:#?}", problems(&t));
    }
    // An out-of-line unit-test module is fine (neither verified nor shipped).
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "#[cfg(test)]\nmod tests {",
        "#[cfg(test)]\nmod more_tests;\n\n#[cfg(test)]\nmod tests {",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

#[test]
fn g9_the_kernel_crate_has_no_macro_use_or_extern_crate() {
    for (find, replace) in [
        ("mod kern;", "#[macro_use]\nmod helpers;\nmod kern;"),
        ("mod kern;", "extern crate core as std2;\nmod kern;"),
    ] {
        let mut t = good();
        edit(&mut t, LIB, find, replace);
        let p = problems(&t);
        assert!(
            p.iter().any(|p| p.guard == "G9" && p.file == LIB),
            "{replace}: {p:#?}"
        );
    }
}

#[test]
fn g1_unregistered_verus_files_and_orphan_specs() {
    let mut t = good();
    t.put(
        "crates/demo/src/looks_verified.rs",
        "#[cfg_attr(verus_keep_ghost, verus_spec(r => ensures r == 1))]\npub fn one() -> u8 { 1 }\n",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G1" && p.file == "crates/demo/src/looks_verified.rs"),
        "{p:#?}"
    );
    let mut t = good();
    edit(
        &mut t,
        "proofs/verus/spec/mod.rs",
        "pub mod demo;",
        "pub mod demo;\npub mod gone;",
    );
    t.put("proofs/verus/spec/gone.rs", "verus! {}\n");
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G1" && p.msg.contains("spec module `gone`")),
        "{p:#?}"
    );
    // A spec mutant names the kernel file or its spec only.
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "mutants = [{ find",
        "mutants = [{ file = \"crates/demo/src/lib.rs\", find",
    );
    assert_eq!(fired(&t), ["G1"]);
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "mutants = [{ find",
        "mutants = [{ file = \"proofs/verus/spec/demo.rs\", find",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

#[test]
fn g9_std_core_alloc_and_local_paths_pass() {
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "x.saturating_add(Scalar::ONE)",
        "core::cmp::min(x, usize::MAX).saturating_add(Scalar::ONE).min(std::usize::MAX).min(Option::Some(1).unwrap_or(1))",
    );
    edit(
        &mut t,
        KERN,
        "use std::ops::Range;",
        "use std::ops::Range;\nuse core::cmp::Ordering as _;",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

#[test]
fn g9_ignores_unit_test_modules() {
    let mut t = good();
    edit(
        &mut t,
        KERN,
        "    use super::*;\n",
        "    use super::*;\n    use crate::fast::Fast;\n",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
}

// G10 -----------------------------------------------------------------------------------------------------------

#[test]
fn g10_callers_must_exist_and_call_the_kernel() {
    let mut t = good();
    edit(&mut t, REGISTRY, "lib.rs::split", "lib.rs::split_all");
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G10" && p.msg.contains("`split_all`")),
        "{p:#?}"
    );
    let mut t = good();
    edit(
        &mut t,
        LIB,
        "kern::spans(&fast::Fast, b'\\n', hay)",
        "vec![0..hay.len()]",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G10" && p.msg.contains("does not call kernel")),
        "{p:#?}"
    );
}

#[test]
fn g10_stale_and_placeholder_anchors() {
    let mut t = good();
    edit(&mut t, LIB, "b'\\n'", "b'\\t'");
    let p = problems(&t);
    assert_eq!(p.len(), 1, "{p:#?}");
    assert!(p[0].msg.contains("model link stale"), "{p:#?}");
    // A doc-comment or formatting edit keeps the link.
    let mut t = good();
    edit(
        &mut t,
        LIB,
        "/// Splits `hay` at newlines",
        "/// Splits the bytes `hay` at newlines",
    );
    edit(
        &mut t,
        LIB,
        "    kern::spans(&fast::Fast, b'\\n', hay)\n",
        "    kern::spans(\n        &fast::Fast,\n        b'\\n',\n        hay,\n    )\n",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
    let mut t = fixture();
    edit(&mut t, REGISTRY, "0000000000000000", "TBD");
    assert_eq!(fired(&t), ["G10"]);
}

#[test]
fn g10_driver_and_oracle_test_are_fingerprinted() {
    let mut t = good();
    edit(
        &mut t,
        "proofs/lean/DemoMain.lean",
        "IO.println \"ok\"",
        "IO.println \"OK\"",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G10" && p.msg.contains("oracle driver")),
        "{p:#?}"
    );
    let mut t = good();
    edit(
        &mut t,
        "crates/demo/tests/oracle.rs",
        "fn oracle_agrees() {}",
        "fn oracle_agrees() {\n    return;\n}",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G10" && p.msg.contains("oracle test")),
        "{p:#?}"
    );
    // Comments in the oracle test do not count.
    let mut t = good();
    edit(
        &mut t,
        "crates/demo/tests/oracle.rs",
        "fn oracle_agrees() {}",
        "fn oracle_agrees() {} // nothing to do",
    );
    assert_eq!(problems(&t), Vec::<Problem>::new());
    // Every theorem has a statement pin, and nothing else does.
    let mut t = good();
    edit(&mut t, REGISTRY, "\"Demo.t\" = ", "\"Demo.u\" = ");
    assert_eq!(fired(&t), ["G10"]);
}

#[test]
fn g10_model_files_and_oracles() {
    let mut t = good();
    t.remove("proofs/lean/Demo.lean");
    assert_eq!(fired(&t), ["G10"]);
    let mut t = good();
    edit(
        &mut t,
        "proofs/lean/lakefile.toml",
        "name = \"demo-model\"",
        "name = \"other-model\"",
    );
    assert_eq!(fired(&t), ["G10"]);
    let mut t = good();
    edit(
        &mut t,
        REGISTRY,
        "lib.rs::split\", fingerprint",
        "lib.rs::nope\", fingerprint",
    );
    let p = problems(&t);
    assert!(
        p.iter()
            .any(|p| p.guard == "G10" && p.msg.contains("no item named `nope`")),
        "{p:#?}"
    );
}

/// A protocol model (K9) has no oracle: it names the behaviour tests that witness its scenarios, and each must exist.
#[test]
fn g10_a_model_without_an_oracle_names_existing_witnesses() {
    let witnessed = |w: &str| {
        let mut t = good();
        let reg = t.read(REGISTRY).unwrap();
        let mut out = String::new();
        for line in reg.lines() {
            if [
                "oracle =",
                "oracle_test =",
                "driver_fingerprint =",
                "oracle_test_fingerprint =",
            ]
            .iter()
            .any(|k| line.starts_with(k))
            {
                continue;
            }
            out.push_str(line);
            out.push('\n');
            if line.starts_with("theorems =") && !w.is_empty() {
                out.push_str("witnesses = [\"");
                out.push_str(w);
                out.push_str("\"]\n");
            }
        }
        t.put(REGISTRY, &out);
        t
    };
    assert_eq!(
        problems(&witnessed("crates/demo/tests/oracle.rs::oracle_agrees")),
        Vec::<Problem>::new()
    );
    let p = problems(&witnessed("crates/demo/tests/oracle.rs::no_such_test"));
    assert!(
        p.len() == 1 && p[0].guard == "G10" && p[0].msg.contains("witness"),
        "{p:#?}"
    );
    let p = problems(&witnessed(""));
    assert!(
        p.len() == 1 && p[0].guard == "G10" && p[0].msg.contains("witnesses"),
        "{p:#?}"
    );
}

// The V2 manifest -----------------------------------------------------------------------------------------------

#[test]
fn the_manifest_lists_every_function_but_unit_tests() {
    let t = good();
    let fns = kernel_fns(&t.read(KERN).unwrap()).unwrap();
    let names: Vec<&str> = fns.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "FindByte::find_byte",
            "Scalar::find_byte",
            "spans",
            "helper"
        ]
    );
}
