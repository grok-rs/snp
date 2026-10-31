//! P0 natives against the Python hooks (WP-26 acceptance): the per-epoch goldens of `cases.json` recorded by
//! `cargo xtask pch-goldens p0` under the pinned pre-commit-hooks venvs (v3.2.0, v4.0.1, v4.4.0, v4.6.0, v5.0.0,
//! v6.0.0; CPython 3.13), partition-emulated cases included, and the Appendix A argparse goldens of every P0
//! console script of every supported tag.
//!
//! Owner: WP-26.
#![cfg(test)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::{OsStr, OsString};

use snp_hooks::{Epoch, NativeSource, Outcome};
use support::{Ctx, Sandbox, argv_of, epoch_at, latin1, show};

/// The P0 hooks and their console scripts.
const P0: [(&str, &str); 10] = [
    ("trailing-whitespace", "trailing-whitespace-fixer"),
    ("end-of-file-fixer", "end-of-file-fixer"),
    ("mixed-line-ending", "mixed-line-ending"),
    ("fix-byte-order-marker", "fix-byte-order-marker"),
    ("check-merge-conflict", "check-merge-conflict"),
    ("detect-private-key", "detect-private-key"),
    ("check-added-large-files", "check-added-large-files"),
    ("no-commit-to-branch", "no-commit-to-branch"),
    ("check-json", "check-json"),
    (
        "check-executables-have-shebangs",
        "check-executables-have-shebangs",
    ),
];

/// Runs one case natively: `(code, output, crashed, files after, modified)`.
fn run_case(case: &serde_json::Value, tag: &str) -> (Outcome, support::Files, Vec<String>) {
    let hook_id = case["hook"].as_str().unwrap();
    let epoch = epoch_at(tag, hook_id).unwrap_or_else(|| panic!("{hook_id} has no epoch at {tag}"));
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, hook_id).unwrap();
    assert_eq!(hook.prog(), case["script"].as_str().unwrap());
    assert!(
        hook.preflight(epoch, &[]).is_ok(),
        "{hook_id} delegates at {tag}"
    );
    let sb = Sandbox::new(true);
    let written = sb.steps(&case["steps"]);
    if let Some(big) = case.get("big").and_then(|b| b.as_object()) {
        for (path, size) in big {
            sb.write(
                path,
                &vec![b'B'; usize::try_from(size.as_u64().unwrap()).unwrap()],
            );
        }
    }
    if let Some(big) = case.get("bigint").and_then(|b| b.as_object()) {
        for (path, digits) in big {
            sb.write(
                path,
                &vec![b'1'; usize::try_from(digits.as_u64().unwrap()).unwrap()],
            );
        }
    }
    let paths: Vec<String> = match case.get("files").and_then(|f| f.as_array()) {
        Some(f) => f.iter().map(|p| p.as_str().unwrap().to_owned()).collect(),
        None => written,
    };
    let before = sb.snapshot(&paths);
    let ctx = Ctx::new(sb);
    let cx = ctx.cx(epoch);
    let argv = argv_of(&case["argv"]);
    let outcome = if let Some(part) = case.get("partition") {
        let files = argv_of(&part["files"]);
        let order = snp_pycompat::random::precommit_shuffled_indices(files.len());
        let shuffled: Vec<&OsStr> = order
            .iter()
            .map(|&i| files[usize::try_from(i).unwrap()].as_os_str())
            .collect();
        let jobs =
            std::num::NonZeroUsize::new(usize::try_from(part["jobs"].as_u64().unwrap()).unwrap())
                .unwrap();
        let ranges = snp_proc::xargs::partition(&[], &shuffled, jobs, 131_072).unwrap();
        let mut total = Outcome::default();
        for r in ranges {
            let mut full: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();
            full.extend_from_slice(&shuffled[r]);
            let o = hook.run(&cx, &full);
            if o.code.unsigned_abs() > total.code.unsigned_abs() {
                total.code = o.code;
            }
            total.output.extend_from_slice(&o.output);
            total.crashed |= o.crashed;
            total.modified.extend(o.modified);
        }
        total
    } else {
        let full: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();
        hook.run(&cx, &full)
    };
    let after = ctx.sb.snapshot(&paths);
    let changed: Vec<String> = before
        .iter()
        .zip(after.iter())
        .filter(|(b, a)| b.1 != a.1)
        .map(|(b, _)| b.0.clone())
        .collect();
    (outcome, after, changed)
}

/// Lines of `out`, sorted (cases whose Python iterates a `set`).
fn sorted_lines(out: &[u8]) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = out.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    v.sort();
    v
}

#[cfg(unix)]
#[test]
fn p0_goldens_per_epoch() {
    let cases = support::json("p0/cases.json");
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    for tag in cases["tags"].as_array().unwrap() {
        let tag = tag.as_str().unwrap();
        let goldens = support::json(&format!("p0/goldens/{tag}.json"));
        for case in cases["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let Some(want) = goldens["results"].get(name) else {
                continue;
            };
            let (got, after, changed) = run_case(case, tag);
            checked += 1;
            let want_out = latin1(want["out"].as_str().unwrap());
            let want_code = i32::try_from(want["code"].as_i64().unwrap()).unwrap();
            let unordered = case
                .get("unordered")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let out_ok = if unordered {
                sorted_lines(&got.output) == sorted_lines(&want_out)
            } else {
                got.output == want_out
            };
            if !out_ok || got.code != want_code || got.crashed != want["crashed"].as_bool().unwrap()
            {
                failures.push(format!(
                    "{tag} {name}: code {} (want {want_code}), crashed {} (want {})\n--- got\n{}\n--- want\n{}",
                    got.code,
                    got.crashed,
                    want["crashed"],
                    show(&got.output),
                    show(&want_out)
                ));
            }
            for (path, bytes) in &after {
                let want_bytes = want["files"].get(path).and_then(|v| v.as_str()).map(latin1);
                if *bytes != want_bytes {
                    failures.push(format!(
                        "{tag} {name}: {path} = {:?} (want {:?})",
                        bytes.as_deref().map(show),
                        want_bytes.as_deref().map(show)
                    ));
                }
            }
            // The natives report exactly the files they rewrote (§3.7).
            let mut modified: Vec<String> = got
                .modified
                .iter()
                .map(|p| support::to_latin1(p.as_bytes()))
                .collect();
            modified.sort();
            modified.dedup();
            let mut changed = changed;
            changed.sort();
            if modified != changed {
                failures.push(format!(
                    "{tag} {name}: modified {modified:?} but changed {changed:?}"
                ));
            }
        }
    }
    assert!(checked > 300, "only {checked} golden cases ran");
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// The Appendix A texts (`-h` → stdout, exit 0; an unknown option → usage + error, exit 2) of every P0 console
/// script of every supported tag, rendered by the native's parser at that tag's epoch.
#[cfg(unix)]
#[test]
fn p0_appendix_a_goldens() {
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    let sb = Sandbox::new(true);
    let ctx = Ctx::new(sb);
    for profile in tags() {
        let golden = support::json(&format!("goldens/appendix_a/{profile}.json"));
        for (hook_id, script) in P0 {
            let Some(want) = golden["scripts"].get(script) else {
                continue;
            };
            let Some(epoch) = epoch_at(profile, hook_id) else {
                continue;
            };
            let hook = snp_hooks::native(NativeSource::PreCommitHooks, hook_id).unwrap();
            let cx = ctx.cx(epoch);
            for (kind, arg) in [("help", "-h"), ("usage", "--snp-no-such-option")] {
                let w = &want[kind];
                let got = hook.run(&cx, &[OsStr::new(arg)]);
                checked += 1;
                let want_out = latin1(w["out"].as_str().unwrap());
                if got.output != want_out || i64::from(got.code) != w["code"].as_i64().unwrap() {
                    failures.push(format!(
                        "{profile} {script} {kind}: code {}\n--- got\n{}\n--- want\n{}",
                        got.code,
                        show(&got.output),
                        show(&want_out)
                    ));
                }
            }
        }
    }
    assert!(checked >= 250, "only {checked} Appendix A texts compared");
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Every supported tag.
fn tags() -> [&'static str; 16] {
    [
        "v3.0.0", "v3.0.1", "v3.1.0", "v3.2.0", "v3.3.0", "v3.4.0", "v4.0.0", "v4.0.1", "v4.1.0",
        "v4.2.0", "v4.3.0", "v4.4.0", "v4.5.0", "v4.6.0", "v5.0.0", "v6.0.0",
    ]
}

/// The registry serves every P0 hook under its id and console script, natively at every epoch of every tag.
#[test]
fn p0_registry_and_preflight() {
    support::link_all();
    for (id, script) in P0 {
        let hook = snp_hooks::native(NativeSource::PreCommitHooks, id).unwrap();
        assert_eq!(hook.id(), id);
        assert_eq!(hook.prog(), script);
        assert!(hook.reports_modifications_exactly());
        for tag in tags() {
            if let Some(epoch) = epoch_at(tag, id) {
                assert!(hook.preflight(epoch, &[]).is_ok(), "{id} {tag}");
            }
        }
        assert!(hook.preflight(Epoch(0), &[]).is_err());
        assert!(hook.preflight(Epoch(99), &[]).is_err());
    }
    assert!(snp_hooks::native(NativeSource::PreCommitHooks, "check-ast").is_none());
}

/// `no-commit-to-branch --pattern` beyond snp's regex engines makes the caller delegate; argparse errors and
/// invalid Python patterns are emulated, not delegated.
#[test]
fn no_commit_to_branch_preflight() {
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "no-commit-to-branch").unwrap();
    let args = |xs: &[&str]| {
        xs.iter()
            .map(|&x| std::sync::Arc::<str>::from(x))
            .collect::<Vec<_>>()
    };
    assert!(hook.preflight(Epoch(2), &args(&["-p", "rel.*"])).is_ok());
    assert!(hook.preflight(Epoch(2), &args(&["-p", "("])).is_ok());
    assert!(hook.preflight(Epoch(2), &args(&["--bogus"])).is_ok());
}

/// `rewrite_in_place` keeps the inode, the mode and hard links (09 §2.8).
#[cfg(unix)]
#[test]
fn rewrite_preserves_inode_mode_and_links() {
    use std::os::unix::fs::MetadataExt as _;
    let sb = Sandbox::new(true);
    sb.write("a.txt", b"x \n");
    sb.chmod("a.txt", 0o640);
    std::fs::hard_link(sb.path("a.txt"), sb.path("link.txt")).unwrap();
    let before = std::fs::metadata(sb.path("a.txt")).unwrap();
    let ctx = Ctx::new(sb);
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "trailing-whitespace").unwrap();
    let o = hook.run(&ctx.cx(Epoch(1)), &[OsStr::new("a.txt")]);
    assert_eq!(o.code, 1);
    let after = std::fs::metadata(ctx.sb.path("a.txt")).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(after.mode() & 0o777, 0o640);
    assert_eq!(std::fs::read(ctx.sb.path("link.txt")).unwrap(), b"x\n");
}

/// D-CRASH-23: a read-only file that needs no change passes end-of-file-fixer (Python's `rb+` open crashes);
/// one that needs a change is still the `PermissionError` crash.
#[cfg(unix)]
#[test]
fn readonly_file_without_change_passes() {
    let sb = Sandbox::new(true);
    sb.write("ok.txt", b"done\n");
    sb.write("bad.txt", b"x");
    sb.chmod("ok.txt", 0o444);
    sb.chmod("bad.txt", 0o444);
    let ctx = Ctx::new(sb);
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "end-of-file-fixer").unwrap();
    let o = hook.run(&ctx.cx(Epoch(1)), &[OsStr::new("ok.txt")]);
    assert_eq!(
        (o.code, o.crashed, o.output.as_slice()),
        (0, false, &b""[..])
    );
    // Root may write anything: only assert the crash for an unprivileged user.
    if std::fs::OpenOptions::new()
        .write(true)
        .open(ctx.sb.path("bad.txt"))
        .is_err()
    {
        let o = hook.run(
            &ctx.cx(Epoch(1)),
            &[OsStr::new("bad.txt"), OsStr::new("ok.txt")],
        );
        assert!(o.crashed);
        assert_eq!(
            o.output,
            b"PermissionError: [Errno 13] Permission denied: 'bad.txt'\n"
        );
    }
}

/// A crash stops the partition: later files are not processed, earlier fixes persist.
#[test]
fn crash_stops_the_partition() {
    let sb = Sandbox::new(true);
    sb.write("a.txt", b"a \n");
    sb.write("c.txt", b"c \n");
    let ctx = Ctx::new(sb);
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "trailing-whitespace").unwrap();
    let o = hook.run(
        &ctx.cx(Epoch(1)),
        &[
            OsStr::new("a.txt"),
            OsStr::new("missing"),
            OsStr::new("c.txt"),
        ],
    );
    assert_eq!(o.code, 1);
    assert!(o.crashed);
    assert_eq!(std::fs::read(ctx.sb.path("a.txt")).unwrap(), b"a\n");
    assert_eq!(std::fs::read(ctx.sb.path("c.txt")).unwrap(), b"c \n");
    assert_eq!(o.modified.len(), 1);
}

/// Cancellation is observed between files.
#[test]
fn cancellation_between_files() {
    let sb = Sandbox::new(true);
    sb.write("a.txt", b"a \n");
    let ctx = Ctx::new(sb);
    ctx.cancel()
        .cancel(snp_core::cancel::CancelReason::Interrupt);
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "trailing-whitespace").unwrap();
    let o = hook.run(&ctx.cx(Epoch(1)), &[OsStr::new("a.txt")]);
    assert_eq!(o.code, 0);
    assert_eq!(std::fs::read(ctx.sb.path("a.txt")).unwrap(), b"a \n");
}

/// D-SEC-10: a `--pattern` that exceeds the backtracking limit fails the hook closed (exit 1, the register line);
/// CPython would hang, so the commit never went through either.
#[test]
fn no_commit_to_branch_backtrack_fails_closed() {
    let sb = Sandbox::new(true);
    let branch = format!("{}b", std::iter::repeat_n('a', 40).collect::<String>());
    sb.git(&["checkout", "-qb", branch.as_str()]);
    let ctx = Ctx::new(sb);
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "no-commit-to-branch").unwrap();
    let pattern = "(?:(?=a)a+)+(?<!a)b";
    let o = hook.run(&ctx.cx(Epoch(2)), &[OsStr::new("-p"), OsStr::new(pattern)]);
    assert_eq!(o.code, 1);
    assert!(o.crashed);
    assert_eq!(
        show(&o.output),
        format!(
            "snp: regular expression '{pattern}' exceeded the backtracking limit on '{branch}'\n"
        )
    );
}

/// `re.compile`'s `FutureWarning`s are part of the hook output (D-MISC-04 form); `SNP_PYTHON_WARNINGS=0` silences.
#[test]
fn no_commit_to_branch_future_warnings() {
    let sb = Sandbox::new(true);
    sb.git(&["checkout", "-qb", "dev"]);
    let ctx = Ctx::new(sb);
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, "no-commit-to-branch").unwrap();
    let argv = [
        OsStr::new("-p"),
        OsStr::new("[[:alpha:]]x"),
        OsStr::new("-p"),
        OsStr::new("(?a)\\w+"),
    ];
    let o = hook.run(&ctx.cx(Epoch(2)), &argv);
    assert_eq!(o.code, 1);
    assert_eq!(
        show(&o.output),
        "[WARNING] regex '[[:alpha:]]x': Possible nested set at position 1 (Python FutureWarning)\n"
    );
    let quiet = ctx
        .sb
        .env
        .patched(&snp_core::env::EnvPatch::default().set("SNP_PYTHON_WARNINGS", "0"));
    let cx = snp_hooks::HookCx {
        env: &quiet,
        ..ctx.cx(Epoch(2))
    };
    let o = hook.run(&cx, &argv);
    assert_eq!((o.code, o.output.as_slice()), (1, &b""[..]));
}
