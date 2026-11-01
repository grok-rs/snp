//! P1 natives against the Python hooks (WP-27 acceptance): the per-epoch goldens of `cases.json` recorded by
//! `cargo xtask pch-gates goldens` under the pinned pre-commit-hooks venvs (CPython 3.13; flag-shaped file names,
//! partition-emulated outputs, crash paths and the bug-compat quirks of 09 §0 #5 included), the Appendix A argparse
//! goldens of every P1 console script at every supported tag, and D-CRASH-23 for the `rb+` fixers.
//!
//! Owner: WP-27.
#![cfg(test)]

#[path = "../framework/support.rs"]
mod support;

#[path = "golden.rs"]
mod golden;

use std::ffi::OsStr;

use snp_hooks::{Epoch, NativeSource};
use support::{Ctx, Sandbox, show};

/// The P1 hooks and their console scripts.
const P1: [(&str, &str); 8] = [
    ("pretty-format-json", "pretty-format-json"),
    ("file-contents-sorter", "file-contents-sorter"),
    ("requirements-txt-fixer", "requirements-txt-fixer"),
    ("check-case-conflict", "check-case-conflict"),
    (
        "check-shebang-scripts-are-executable",
        "check-shebang-scripts-are-executable",
    ),
    ("check-symlinks", "check-symlinks"),
    ("destroyed-symlinks", "destroyed-symlinks"),
    ("check-byte-order-marker", "check-byte-order-marker"),
];

#[cfg(unix)]
#[test]
fn p1_goldens_per_epoch() {
    let checked = golden::replay("p1");
    eprintln!("p1: {checked} golden cases");
    assert!(checked > 400, "only {checked} golden cases ran");
}

#[cfg(unix)]
#[test]
fn p1_appendix_a_goldens() {
    let checked = golden::appendix_a(&P1);
    assert!(checked >= 200, "only {checked} Appendix A texts compared");
}

#[test]
fn p1_registry_and_preflight() {
    golden::registry(&P1);
    // pretty-format-json before v4.0.0 printed its diff through text-mode `print`: delegated.
    let pfj = snp_hooks::native(NativeSource::PreCommitHooks, "pretty-format-json").unwrap();
    assert!(pfj.preflight(Epoch(0), &[]).is_err());
    assert!(golden::native_epoch("v3.4.0", "pretty-format-json", &[]).is_none());
    assert!(golden::native_epoch("v4.0.0", "pretty-format-json", &[]).is_some());
    // requirements-txt-fixer v3.0.x cut names at `==` only: delegated.
    assert!(golden::native_epoch("v3.0.1", "requirements-txt-fixer", &[]).is_none());
    assert!(golden::native_epoch("v3.1.0", "requirements-txt-fixer", &[]).is_some());
    // The real check-byte-order-marker runs up to v5.0.0; v6.0.0 is the removed stub.
    assert!(golden::native_epoch("v5.0.0", "check-byte-order-marker", &[]).is_some());
    let stub = snp_hooks::native(NativeSource::PreCommitHooks, "check-byte-order-marker").unwrap();
    assert_eq!(stub.prog(), "pre-commit-hooks-removed");
}

/// D-CRASH-23: a read-only file that needs no change passes the `rb+` fixers (Python's `open(f, 'rb+')` crashes
/// with `PermissionError`); one that does need a change still crashes, at the write, as in Python.
#[cfg(unix)]
#[test]
fn readonly_fixers_without_change_pass() {
    let sb = Sandbox::new(true);
    sb.write("sorted.txt", b"a\nb\n");
    sb.write("unsorted.txt", b"b\na\n");
    sb.write("requirements.txt", b"a\nb\n");
    sb.chmod("sorted.txt", 0o444);
    sb.chmod("unsorted.txt", 0o444);
    sb.chmod("requirements.txt", 0o444);
    let ctx = Ctx::new(sb);
    let writable = std::fs::OpenOptions::new()
        .write(true)
        .open(ctx.sb.path("unsorted.txt"))
        .is_ok();
    for (hook, epoch, file) in [
        ("file-contents-sorter", 5, "sorted.txt"),
        ("requirements-txt-fixer", 4, "requirements.txt"),
    ] {
        let h = snp_hooks::native(NativeSource::PreCommitHooks, hook).unwrap();
        let o = h.run(&ctx.cx(Epoch(epoch)), &[OsStr::new(file)]);
        assert_eq!(
            (o.code, o.crashed, show(&o.output)),
            (0, false, String::new()),
            "{hook}"
        );
    }
    if !writable {
        let h = snp_hooks::native(NativeSource::PreCommitHooks, "file-contents-sorter").unwrap();
        let o = h.run(
            &ctx.cx(Epoch(5)),
            &[OsStr::new("unsorted.txt"), OsStr::new("sorted.txt")],
        );
        assert!(o.crashed);
        assert_eq!(
            show(&o.output),
            "PermissionError: [Errno 13] Permission denied: 'unsorted.txt'\n"
        );
    }
}

/// pretty-format-json writes its diffs through `sys.stdout.buffer` and its messages with `print`: in a pipe
/// the diffs come first unless the pending text exceeds the 8 KiB text chunk.
#[test]
fn pretty_format_json_output_order() {
    let sb = Sandbox::new(true);
    sb.write("bad.json", b"{");
    sb.write("a.json", b"[1]");
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "pretty-format-json").unwrap();
    let o = h.run(
        &ctx.cx(Epoch(2)),
        &[OsStr::new("bad.json"), OsStr::new("a.json")],
    );
    let out = show(&o.output);
    let diff = out.find("--- a.json").unwrap();
    let msg = out.find("Input File bad.json").unwrap();
    assert!(diff < msg, "{out}");
    // v4.x stops at the first invalid file: no diff.
    let o = h.run(
        &ctx.cx(Epoch(1)),
        &[OsStr::new("bad.json"), OsStr::new("a.json")],
    );
    assert_eq!(o.code, 1);
    assert!(!show(&o.output).contains("--- a.json"));
}

/// Cancellation is observed between files.
#[test]
fn cancellation_between_files() {
    let sb = Sandbox::new(true);
    sb.write("a", b"b\na\n");
    let ctx = Ctx::new(sb);
    ctx.cancel()
        .cancel(snp_core::cancel::CancelReason::Interrupt);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "file-contents-sorter").unwrap();
    let o = h.run(&ctx.cx(Epoch(5)), &[OsStr::new("a")]);
    assert_eq!(o.code, 0);
    assert_eq!(std::fs::read(ctx.sb.path("a")).unwrap(), b"b\na\n");
}

/// `rewrite_in_place` keeps the inode and the mode (09 §2.8).
#[cfg(unix)]
#[test]
fn sorter_rewrite_preserves_inode_and_mode() {
    use std::os::unix::fs::MetadataExt as _;
    let sb = Sandbox::new(true);
    sb.write("r.txt", b"b\na\n");
    sb.chmod("r.txt", 0o640);
    let before = std::fs::metadata(sb.path("r.txt")).unwrap();
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "requirements-txt-fixer").unwrap();
    let o = h.run(&ctx.cx(Epoch(4)), &[OsStr::new("r.txt")]);
    assert_eq!(o.code, 1);
    let after = std::fs::metadata(ctx.sb.path("r.txt")).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(after.mode() & 0o777, 0o640);
    assert_eq!(std::fs::read(ctx.sb.path("r.txt")).unwrap(), b"a\nb\n");
}

/// Windows text mode (09 §2.7): `--autofix` writes through `open(f, 'w')`, so every `\n` becomes `\r\n`; a CRLF
/// file that is otherwise pretty passes on every platform (universal newlines on read).
#[test]
fn pretty_format_json_windows_text_writes() {
    let sb = Sandbox::new(true);
    sb.write("a.json", b"{\"a\":1}");
    sb.write("b.json", b"{\r\n  \"a\": 1\r\n}\r\n");
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "pretty-format-json").unwrap();
    let cx = snp_hooks::HookCx {
        windows: true,
        ..ctx.cx(Epoch(2))
    };
    let o = h.run(
        &cx,
        &[
            OsStr::new("--autofix"),
            OsStr::new("a.json"),
            OsStr::new("b.json"),
        ],
    );
    assert_eq!(o.code, 1);
    assert_eq!(o.modified.len(), 1);
    assert_eq!(
        std::fs::read(ctx.sb.path("a.json")).unwrap(),
        b"{\r\n  \"a\": 1\r\n}\r\n"
    );
    assert_eq!(
        std::fs::read(ctx.sb.path("b.json")).unwrap(),
        b"{\r\n  \"a\": 1\r\n}\r\n"
    );
}
