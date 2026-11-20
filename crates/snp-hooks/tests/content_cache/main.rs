//! The content cache of a window of consecutive in-process hooks (ADR-131, "B-racy"): every chain of natives run with
//! a cache produces exactly the outcomes and files of the same chain without one, also when files change between
//! hooks (through a native, an alias or another process), and the cache serves hits only where a fresh read would
//! return the same bytes. Files are aged past the 2 s racy window with one sleep per test (ctime cannot be set back).
//!
//! Owner: WP-26.
#![cfg(test)]
#![cfg(unix)]
#![allow(
    dead_code,
    reason = "the sandbox's shared helpers serve every test target; this one uses a few"
)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::OsStr;
use std::time::Duration;

use snp_hooks::content_cache::{PER_FILE_CAP, RACY_WINDOW, Stats, TOTAL_CAP};
use snp_hooks::{ContentCache, HookCx, NativeSource};
use support::{Ctx, Sandbox, epoch_at};

/// What one hook run returned: code, output, modified paths.
type Ran = (i32, String, Vec<String>);

/// Waits until everything written so far is older than the racy window.
fn age() {
    std::thread::sleep(RACY_WINDOW + Duration::from_millis(300));
}

/// Two identical sandboxes (cached and uncached) holding `files`.
fn twins(files: &[(&str, &[u8])]) -> (Ctx, Ctx) {
    let make = || {
        let sb = Sandbox::new(true);
        for (p, b) in files {
            sb.write(p, b);
        }
        Ctx::new(sb)
    };
    (make(), make())
}

/// A cache over the sandbox, or `None` when the filesystem refuses one (the test then proves nothing and says so).
fn cache_of(ctx: &Ctx, verify: bool) -> Option<ContentCache> {
    let c = ContentCache::new(&ctx.sb.root, verify);
    if c.is_none() {
        eprintln!(
            "{}: not a stat_oracle_safe filesystem, no content cache",
            ctx.sb.root.display()
        );
    }
    c
}

/// Runs the v6.0.0 native `id` with `args` and `files` under `cache`.
fn run(ctx: &Ctx, cache: Option<&ContentCache>, id: &str, args: &[&str], files: &[&str]) -> Ran {
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, id).unwrap();
    let cx = HookCx {
        cache,
        ..ctx.cx(epoch_at("v6.0.0", id).unwrap())
    };
    let argv: Vec<&OsStr> = args.iter().chain(files).map(OsStr::new).collect();
    let o = hook.run(&cx, &argv);
    (
        o.code,
        String::from_utf8_lossy(&o.output).into_owned(),
        o.modified.iter().map(|p| p.lossy().into_owned()).collect(),
    )
}

/// The bytes of `paths` in a sandbox (`None`: unreadable).
fn files(ctx: &Ctx, paths: &[&str]) -> Vec<Option<Vec<u8>>> {
    paths
        .iter()
        .map(|p| std::fs::read(ctx.sb.path(p)).ok())
        .collect()
}

/// The fixture of the fixer-then-checker chains.
const CHAIN: [(&str, &[u8]); 7] = [
    ("a.yaml", b"a: 1  \nb: [2]\r\n"),
    ("b.yaml", b"k: v\n"),
    ("c.json", b"{\"a\": 1}   \n\n\n"),
    ("d.json", b"{\"a\": }\n"),
    ("e.txt", b"no newline at end"),
    ("f.txt", b"mixed\r\nendings\nhere\r\n"),
    ("g.txt", b"clean\n"),
];

/// The chain's hooks: fixers first, then checkers reading what the fixers wrote.
const HOOKS: [&str; 7] = [
    "check-yaml",
    "trailing-whitespace",
    "end-of-file-fixer",
    "mixed-line-ending",
    "check-yaml",
    "check-json",
    "detect-private-key",
];

/// trailing-whitespace → end-of-file-fixer → mixed-line-ending → check-yaml (and friends) on the same files: the
/// cached chain equals the uncached one, hook by hook and in the final bytes, under `SNP_VERIFY_FILE_CACHE` too, and
/// the unchanged files were served from the cache.
#[test]
fn fixer_then_checker_chain_equals_uncached() {
    let names: Vec<&str> = CHAIN.iter().map(|(p, _)| *p).collect();
    let (a, b) = twins(&CHAIN);
    age();
    let Some(cache) = cache_of(&a, true) else {
        return;
    };
    for id in HOOKS {
        let mine: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| !id.starts_with("check-yaml") || n.contains(".yaml"))
            .filter(|n| id != "check-json" || n.contains(".json"))
            .collect();
        let got = run(&a, Some(&cache), id, &[], &mine);
        let want = run(&b, None, id, &[], &mine);
        assert_eq!(got, want, "{id}");
        assert_eq!(files(&a, &names), files(&b, &names), "after {id}");
    }
    let s = cache.stats();
    assert!(
        s.hits >= 10,
        "the chain hit nothing, the test proves nothing: {s:?}"
    );
    assert!(s.fills > 0, "{s:?}");
    assert_eq!(cache.mismatches(), 0);
}

/// Another process edits files between two hooks (same size and new size): the next hook sees the new bytes, exactly
/// like without a cache; the untouched, old files are still served from the cache.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "a test fixture writes files; the write ban (ADR-132) is for native hook code"
)]
fn external_edits_between_hooks_are_seen() {
    // `gone.txt` last: its FileNotFoundError ends each partition
    let fx: [(&str, &[u8]); 4] = [
        ("same.txt", b"abc  \n"),
        ("grow.txt", b"abc\n"),
        ("old.txt", b"keep \n"),
        ("gone.txt", b"x \n"),
    ];
    let names: Vec<&str> = fx.iter().map(|(p, _)| *p).collect();
    let (a, b) = twins(&fx);
    age();
    let Some(cache) = cache_of(&a, true) else {
        return;
    };
    let first = "detect-private-key";
    assert_eq!(
        run(&a, Some(&cache), first, &[], &names),
        run(&b, None, first, &[], &names)
    );
    let filled = cache.stats().fills;
    assert_eq!(filled, 4, "{:?}", cache.stats());
    for ctx in [&a, &b] {
        // a same-size rewrite (racy: just written), a longer one, a deletion
        ctx.sb.write("same.txt", b"xyz  \n");
        ctx.sb.write("grow.txt", b"abc   \n");
        std::fs::remove_file(ctx.sb.path("gone.txt")).unwrap();
    }
    for id in ["trailing-whitespace", "end-of-file-fixer", "check-json"] {
        let got = run(&a, Some(&cache), id, &[], &names);
        let want = run(&b, None, id, &[], &names);
        assert_eq!(got, want, "{id}");
        assert_eq!(files(&a, &names), files(&b, &names), "after {id}");
    }
    let s = cache.stats();
    // trailing-whitespace found the three edited entries stale and hit old.txt (then rewrote it)
    assert!(s.stale >= 3, "{s:?}");
    assert!(s.hits >= 1, "{s:?}");
    assert_eq!(cache.mismatches(), 0);
}

/// An edit to an old file (3 s and more): the first hook fills, the file is rewritten by another process with the same
/// size, the next hook sees the new bytes (its ctime moved past the stored stamp).
#[test]
fn same_size_external_edit_of_an_old_file_is_seen() {
    let (a, b) = twins(&[("f.txt", b"one  \n")]);
    age();
    let Some(cache) = cache_of(&a, false) else {
        return;
    };
    let f = ["f.txt"];
    assert_eq!(
        run(&a, Some(&cache), "check-json", &[], &f),
        run(&b, None, "check-json", &[], &f)
    );
    assert_eq!(cache.stats().fills, 1);
    for ctx in [&a, &b] {
        ctx.sb.write("f.txt", b"two  \n");
    }
    let got = run(&a, Some(&cache), "trailing-whitespace", &[], &f);
    assert_eq!(got, run(&b, None, "trailing-whitespace", &[], &f));
    assert_eq!(files(&a, &f), vec![Some(b"two\n".to_vec())]);
    assert_eq!(cache.stats().stale, 1);
}

/// A hard link (an alias, as a case or normalization twin on APFS would be): both names are cached, a same-size fixer
/// rewrites one, a later hook reading the other name sees the new bytes.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "a test fixture writes files; the write ban (ADR-132) is for native hook code"
)]
fn same_size_alias_write_is_seen() {
    let make = || {
        let sb = Sandbox::new(true);
        sb.write("a.txt", b"b\na\n");
        std::fs::hard_link(sb.path("a.txt"), sb.path("twin.txt")).unwrap();
        Ctx::new(sb)
    };
    let (a, b) = (make(), make());
    age();
    let Some(cache) = cache_of(&a, true) else {
        return;
    };
    let both = ["a.txt", "twin.txt"];
    let id = "detect-private-key";
    assert_eq!(
        run(&a, Some(&cache), id, &[], &both),
        run(&b, None, id, &[], &both)
    );
    assert_eq!(cache.stats().fills, 2);
    // the sorter rewrites `a.txt` in place, same size ("a\nb\n")
    let id = "file-contents-sorter";
    let one = ["a.txt"];
    assert_eq!(
        run(&a, Some(&cache), id, &[], &one),
        run(&b, None, id, &[], &one)
    );
    // on the twin, a stale read would sort again and report a change
    let twin = ["twin.txt"];
    let got = run(&a, Some(&cache), id, &[], &twin);
    assert_eq!(got, run(&b, None, id, &[], &twin));
    assert_eq!(got.0, 0, "{got:?}");
    assert_eq!(files(&a, &both), files(&b, &both));
    assert_eq!(cache.mismatches(), 0);
}

/// The caps: a file over 1 MiB is not kept, one of exactly 1 MiB is, and at most 16 MiB are kept at once.
#[test]
fn caps_bound_the_cache() {
    let sb = Sandbox::new(true);
    let big = vec![b'x'; PER_FILE_CAP];
    let over = vec![b'x'; PER_FILE_CAP + 1];
    let n = TOTAL_CAP / PER_FILE_CAP + 1;
    let mut names = Vec::new();
    for i in 0..n {
        let name = format!("big{i:02}.txt");
        sb.write(&name, &big);
        names.push(name);
    }
    sb.write("over.txt", &over);
    let ctx = Ctx::new(sb);
    age();
    let Some(cache) = cache_of(&ctx, false) else {
        return;
    };
    run(&ctx, Some(&cache), "detect-private-key", &[], &["over.txt"]);
    assert_eq!(
        cache.stats(),
        Stats {
            refused: 1,
            ..Stats::default()
        }
    );
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    run(&ctx, Some(&cache), "detect-private-key", &[], &refs);
    let s = cache.stats();
    assert_eq!(
        (s.fills, s.refused),
        (u64::try_from(n - 1).unwrap(), 2),
        "{s:?}"
    );
    // a second reader hits the kept files and reads the rest
    run(&ctx, Some(&cache), "trailing-whitespace", &[], &refs);
    assert_eq!(cache.stats().hits, u64::try_from(n - 1).unwrap());
}

/// Errors are never cached, a file that is not regular is not kept, a file whose permissions change after the fill
/// gives the same error as without a cache, and a file on another device (through a symlink) is not kept.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "a test fixture writes files; the write ban (ADR-132) is for native hook code"
)]
fn errors_non_regular_files_and_other_devices_are_not_cached() {
    let fx: [(&str, &[u8]); 2] = [("locked.txt", b"secret \n"), ("ok.txt", b"ok\n")];
    let (a, b) = twins(&fx);
    // another device: /dev/shm is a tmpfs on most Linux systems
    let shm = tempfile::tempdir_in("/dev/shm").ok();
    for (i, ctx) in [&a, &b].into_iter().enumerate() {
        std::fs::create_dir_all(ctx.sb.path("dir")).unwrap();
        std::os::unix::fs::symlink("/dev/null", ctx.sb.path("null")).unwrap();
        if let Some(d) = &shm {
            let target = d.path().join(format!("far{i}.txt"));
            std::fs::write(&target, b"far \n").unwrap();
            std::os::unix::fs::symlink(&target, ctx.sb.path("far.txt")).unwrap();
        }
    }
    age();
    let Some(cache) = cache_of(&a, true) else {
        return;
    };
    let id = "detect-private-key";
    for name in ["dir", "missing", "null", "ok.txt", "locked.txt"] {
        assert_eq!(
            run(&a, Some(&cache), id, &[], &[name]),
            run(&b, None, id, &[], &[name]),
            "{name}"
        );
    }
    let s = cache.stats();
    assert_eq!((s.fills, s.refused), (2, 1), "{s:?}");
    if let Some(d) = &shm {
        use std::os::unix::fs::MetadataExt as _;
        let other = std::fs::metadata(d.path()).unwrap().dev()
            != std::fs::metadata(&a.sb.root).unwrap().dev();
        run(&a, Some(&cache), id, &[], &["far.txt"]);
        if other {
            assert_eq!(cache.stats().refused, 2, "{:?}", cache.stats());
        }
    }
    for ctx in [&a, &b] {
        ctx.sb.chmod("locked.txt", 0o000);
    }
    for id in ["trailing-whitespace", "detect-private-key"] {
        for name in ["dir", "missing", "locked.txt", "ok.txt"] {
            assert_eq!(
                run(&a, Some(&cache), id, &[], &[name]),
                run(&b, None, id, &[], &[name]),
                "{id} {name}"
            );
        }
    }
    for ctx in [&a, &b] {
        ctx.sb.chmod("locked.txt", 0o644);
    }
    assert!(cache.stats().hits >= 2, "{:?}", cache.stats());
    assert_eq!(cache.mismatches(), 0);
}

/// end-of-file-fixer and check-executables-have-shebangs read cached files (the tail through a `Cursor`, the head as
/// a prefix) and agree with their uncached runs, fixes included.
#[test]
fn tail_and_head_readers_agree() {
    let fx: [(&str, &[u8]); 5] = [
        ("a.txt", b"x"),
        ("b.txt", b"x\n\n\n"),
        ("c.txt", b"x\r\n\r\n"),
        ("d.txt", b""),
        ("e.sh", b"#!/bin/sh\n"),
    ];
    let names: Vec<&str> = fx.iter().map(|(p, _)| *p).collect();
    let (a, b) = twins(&fx);
    for ctx in [&a, &b] {
        ctx.sb.chmod("e.sh", 0o755);
        ctx.sb.chmod("a.txt", 0o755);
        ctx.sb.git(&["add", "."]);
    }
    age();
    let Some(cache) = cache_of(&a, true) else {
        return;
    };
    for id in [
        "detect-private-key",
        "check-executables-have-shebangs",
        "end-of-file-fixer",
        "check-shebang-scripts-are-executable",
    ] {
        assert_eq!(
            run(&a, Some(&cache), id, &[], &names),
            run(&b, None, id, &[], &names),
            "{id}"
        );
        assert_eq!(files(&a, &names), files(&b, &names), "after {id}");
    }
    assert!(cache.stats().hits >= 5, "{:?}", cache.stats());
    assert_eq!(cache.mismatches(), 0);
}

/// pygrep fills and reads the cache too (`run_partition_cached`).
#[test]
fn pygrep_uses_the_cache() {
    let (a, b) = twins(&[("a.py", b"x = 1  # TODO\n"), ("b.py", b"y = 2\n")]);
    age();
    let Some(cache) = cache_of(&a, true) else {
        return;
    };
    let regexes = snp_pyre::RegexCache::default();
    let compiled = snp_hooks::pygrep::prepare(&[], "TODO", 80).unwrap();
    let argv: Vec<&OsStr> = ["TODO", "a.py", "b.py"].map(OsStr::new).to_vec();
    let cancel = snp_core::cancel::CancelToken::new();
    for _ in 0..2 {
        let got = snp_hooks::pygrep::run_partition_cached(
            &compiled,
            &argv,
            &a.sb.root,
            &regexes,
            &cancel,
            Some(&cache),
        );
        let want =
            snp_hooks::pygrep::run_partition(&compiled, &argv, &b.sb.root, &regexes, &cancel);
        assert_eq!((got.code, got.output), (want.code, want.output));
    }
    assert_eq!(cache.stats().hits, 2, "{:?}", cache.stats());
    assert_eq!(cache.mismatches(), 0);
}

/// Files written just now are racy: nothing is kept, so a chain over fresh files never hits.
#[test]
fn racy_files_are_not_kept() {
    let (a, b) = twins(&CHAIN);
    let Some(cache) = cache_of(&a, false) else {
        return;
    };
    let names: Vec<&str> = CHAIN.iter().map(|(p, _)| *p).collect();
    for id in ["detect-private-key", "trailing-whitespace"] {
        assert_eq!(
            run(&a, Some(&cache), id, &[], &names),
            run(&b, None, id, &[], &names)
        );
    }
    let s = cache.stats();
    assert_eq!((s.hits, s.fills), (0, 0), "{s:?}");
}
