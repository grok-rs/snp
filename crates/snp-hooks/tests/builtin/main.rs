//! `repo: builtin` variants (09 §7, ARCHITECTURE §7.3): the registry under `NativeSource::Builtin`, the manifest
//! defaults of `snp_hooks_data::BUILTIN_MANIFEST_YAML`, the `--check` flags and the upstream fixes of 09 §7.2.
//!
//! Owner: WP-26.
#![cfg(test)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::OsStr;

use snp_hooks::{Epoch, NativeSource, Outcome};
use support::{Ctx, Sandbox, show};

/// The P0 ids with a dedicated builtin variant.
const P0: [&str; 10] = [
    "trailing-whitespace",
    "end-of-file-fixer",
    "mixed-line-ending",
    "fix-byte-order-marker",
    "check-merge-conflict",
    "detect-private-key",
    "check-added-large-files",
    "no-commit-to-branch",
    "check-json",
    "check-executables-have-shebangs",
];

/// `(id, entry)` of every `language: python` hook of the builtin manifest.
fn manifest() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut id = None;
    for line in snp_hooks_data::BUILTIN_MANIFEST_YAML.lines() {
        if let Some(i) = line.strip_prefix("-   id: ") {
            id = Some(i.trim().to_owned());
        } else if let (Some(e), Some(i)) = (line.trim().strip_prefix("entry: "), id.as_ref()) {
            out.push((i.clone(), e.to_owned()));
        } else if line.trim() == "language: fail" {
            out.retain(|(i, _)| Some(i) != id.as_ref());
        }
    }
    out
}

fn builtin(id: &str) -> &'static dyn snp_hooks::NativeHook {
    snp_hooks::native(NativeSource::Builtin, id).unwrap()
}

/// Runs a builtin over `argv` in `sb`'s work tree.
fn run(sb: Sandbox, id: &str, argv: &[&str]) -> (Outcome, Ctx) {
    let ctx = Ctx::new(sb);
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let o = builtin(id).run(&ctx.cx(Epoch(1)), &argv);
    (o, ctx)
}

fn read(ctx: &Ctx, rel: &str) -> Vec<u8> {
    std::fs::read(ctx.sb.path(rel)).unwrap()
}

/// Every P0 id resolves to its builtin variant (prog = the manifest entry, every epoch accepted); the builtin
/// ids without a dedicated variant fall back to the pre-commit-hooks native when one is registered; ids outside
/// the builtin manifest are unknown.
#[test]
fn registry_and_manifest_defaults() {
    support::link_all();
    let manifest = manifest();
    for id in P0 {
        let (_, entry) = manifest.iter().find(|(i, _)| i == id).unwrap();
        let h = builtin(id);
        assert_eq!(h.id(), id);
        assert_eq!(h.prog(), entry, "{id}");
        assert!(h.preflight(Epoch(0), &[]).is_ok());
        assert!(!std::ptr::addr_eq(
            h,
            snp_hooks::native(NativeSource::PreCommitHooks, id).unwrap()
        ));
    }
    for (id, entry) in &manifest {
        if let Some(h) = snp_hooks::native(NativeSource::Builtin, id) {
            assert_eq!(h.prog(), entry, "{id}");
        }
    }
    assert!(snp_hooks::native(NativeSource::Builtin, "check-ast").is_none());
    assert!(snp_hooks::native(NativeSource::Builtin, "autopep8-wrapper").is_none());
}

/// trailing-whitespace: `x.` is not markdown by default (upstream's `md_exts == ['.']`), `--check` writes nothing.
#[test]
fn trailing_whitespace_fixes_and_check() {
    let sb = Sandbox::new(true);
    sb.write("d.", b"x  \n");
    let (o, ctx) = run(sb, "trailing-whitespace", &["d."]);
    assert_eq!((o.code, show(&o.output).as_str()), (1, "Fixing d.\n"));
    assert_eq!(read(&ctx, "d."), b"x\n");
    let sb = Sandbox::new(true);
    sb.write("a.txt", b"a \n");
    let (o, ctx) = run(sb, "trailing-whitespace", &["--check", "a.txt"]);
    assert_eq!((o.code, show(&o.output).as_str()), (1, "Would fix a.txt\n"));
    assert!(o.reads_only && o.modified.is_empty());
    assert_eq!(read(&ctx, "a.txt"), b"a \n");
}

/// end-of-file-fixer, mixed-line-ending, fix-byte-order-marker: `--check`; the BOM fixer is byte-level.
#[test]
fn fixers_check_and_byte_level_bom() {
    let sb = Sandbox::new(true);
    sb.write("a", b"x");
    let (o, ctx) = run(sb, "end-of-file-fixer", &["--check", "a"]);
    assert_eq!((o.code, show(&o.output).as_str()), (1, "Would fix a\n"));
    assert_eq!(read(&ctx, "a"), b"x");

    let sb = Sandbox::new(true);
    sb.write("m", b"a\r\nb\n");
    let (o, ctx) = run(sb, "mixed-line-ending", &["--check", "m"]);
    assert_eq!(
        (o.code, show(&o.output).as_str()),
        (1, "m: mixed line endings\n")
    );
    assert_eq!(read(&ctx, "m"), b"a\r\nb\n");

    let sb = Sandbox::new(true);
    sb.write("b", b"\xef\xbb\xbfab\xe9x");
    let (o, ctx) = run(sb, "fix-byte-order-marker", &["b"]);
    assert_eq!((o.code, o.crashed), (1, false));
    assert_eq!(show(&o.output), "b: removed byte-order marker\n");
    assert_eq!(read(&ctx, "b"), b"ab\xe9x");
}

/// check-json: a document nested deeper than CPython allows is reported, not a crash ending the partition.
#[test]
fn check_json_deep_nesting_is_reported() {
    let sb = Sandbox::new(true);
    sb.write("deep.json", &vec![b'['; 20_000]);
    sb.write("bad.json", b"{");
    let (o, _ctx) = run(sb, "check-json", &["deep.json", "bad.json"]);
    assert!(!o.crashed);
    assert_eq!(
        show(&o.output),
        "deep.json: Failed to json decode (maximum nesting depth exceeded)\nbad.json: Failed to json decode \
         (Expecting property name enclosed in double quotes: line 1 column 2 (char 1))\n"
    );
}

/// check-added-large-files: `-z` plumbing checks non-ASCII names; `--include-renames` counts staged renames.
#[test]
fn large_files_non_ascii_and_renames() {
    let sb = Sandbox::new(true);
    sb.write("caf\u{c3}\u{a9}.bin", &[b'B'; 2048]); // latin-1 of the UTF-8 name
    sb.git(&["add", "-A"]);
    let (o, _ctx) = run(
        sb,
        "check-added-large-files",
        &["--maxkb=1", "caf\u{e9}.bin"],
    );
    assert_eq!(show(&o.output), "caf\u{e9}.bin (2 KB) exceeds 1 KB.\n");
    let sb = Sandbox::new(true);
    sb.write("old.bin", &[b'B'; 2048]);
    sb.git(&["add", "old.bin"]);
    sb.git(&["commit", "-qm", "c"]);
    sb.git(&["mv", "old.bin", "new.bin"]);
    let (o, ctx) = run(sb, "check-added-large-files", &["--maxkb=1", "new.bin"]);
    assert_eq!(o.code, 0);
    let o = builtin("check-added-large-files").run(
        &ctx.cx(Epoch(1)),
        &[
            OsStr::new("--maxkb=1"),
            OsStr::new("--include-renames"),
            OsStr::new("new.bin"),
        ],
    );
    assert_eq!(show(&o.output), "new.bin (2 KB) exceeds 1 KB.\n");
}

/// check-merge-conflict: cherry-pick and revert conflicts count as merges; `--diff3` reports `||||||| `.
#[test]
fn merge_conflict_states_and_diff3() {
    for head in ["CHERRY_PICK_HEAD", "REVERT_HEAD"] {
        let sb = Sandbox::new(true);
        sb.write("a.txt", b"<<<<<<< HEAD\n");
        std::fs::write(sb.root.join(".git/MERGE_MSG"), b"m\n").unwrap();
        std::fs::write(sb.root.join(".git").join(head), b"0\n").unwrap();
        let (o, _ctx) = run(sb, "check-merge-conflict", &["a.txt"]);
        assert_eq!(
            show(&o.output),
            "a.txt:1: Merge conflict string '<<<<<<<' found\n",
            "{head}"
        );
    }
    let sb = Sandbox::new(true);
    sb.write("a.txt", b"||||||| base\n");
    let (o, _ctx) = run(
        sb,
        "check-merge-conflict",
        &["--assume-in-merge", "--diff3", "a.txt"],
    );
    assert_eq!(
        show(&o.output),
        "a.txt:1: Merge conflict string '|||||||' found\n"
    );
}

/// check-executables-have-shebangs: pathspecs are literal (`[ab].sh` does not match `a.sh`).
#[test]
fn executables_literal_pathspecs() {
    let sb = Sandbox::new(true);
    sb.git(&["config", "core.fileMode", "false"]);
    sb.write("a.sh", b"echo");
    sb.write("[ab].sh", b"#!/bin/sh\n");
    sb.git(&["add", "a.sh", "[ab].sh"]);
    sb.git(&["update-index", "--chmod=+x", "a.sh"]);
    let (o, ctx) = run(sb, "check-executables-have-shebangs", &["[ab].sh"]);
    assert_eq!((o.code, o.output.len()), (0, 0));
    // Upstream (epoch 5) passes the name as a glob and reports `a.sh`.
    let up = snp_hooks::native(
        NativeSource::PreCommitHooks,
        "check-executables-have-shebangs",
    )
    .unwrap();
    let o = up.run(&ctx.cx(Epoch(5)), &[OsStr::new("[ab].sh")]);
    assert_eq!(o.code, 1);
    assert!(show(&o.output).starts_with("a.sh: marked executable"));
}
