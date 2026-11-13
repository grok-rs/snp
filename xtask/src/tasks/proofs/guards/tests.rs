//! Guard unit tests on the fixture tree `fixtures/good` (a registry, a root, a spec, a Lean package and a demo crate
//! with one kernel, one trusted impl and one model). Each bad case edits the tree in memory and expects its guard.

use std::path::Path;

use super::*;

fn fixture() -> Tree {
    Tree::disk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tasks/proofs/fixtures/good"))
}

const KERN: &str = "crates/demo/src/kern.rs";
const LIB: &str = "crates/demo/src/lib.rs";

/// The fixture with its anchor fingerprint filled in (computed, so editing the fixture's caller stays easy).
fn good() -> Tree {
    let mut t = fixture();
    let fp = fingerprint::of_anchor(&t, "crates/demo/src/lib.rs::split").unwrap();
    edit(&mut t, REGISTRY, "0000000000000000", &fp);
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
fn the_fixture_ships_a_placeholder_fingerprint_that_g10_rejects() {
    let p = problems(&fixture());
    assert_eq!(p.len(), 1, "{p:#?}");
    assert_eq!(p[0].guard, "G10");
    assert!(p[0].msg.contains("model link stale"), "{}", p[0].msg);
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
        (
            "            i = i.saturating_add(1);\n",
            "            i = i.saturating_add(usize::from(matches!(1, 1 if cfg!(unix))));\n",
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert_eq!(fired(&t), ["G2"], "{replace}");
    }
}

// G3 ------------------------------------------------------------------------------------------------------------

#[test]
fn g3_ghost_cfg_only_on_allowed_uses_and_proof_statements() {
    for (find, replace) in [
        ("fn helper(", "#[cfg(verus_keep_ghost)]\nfn helper("),
        ("use crate::spec::demo::*;", "use crate::fast::*;"),
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
        (
            "                return out;\n",
            "                let _ = matches!(sep, crate::SEP);\n                return out;\n",
        ),
    ] {
        let mut t = good();
        edit(&mut t, KERN, find, replace);
        assert_eq!(fired(&t), ["G9"], "{replace}");
    }
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
