//! Golden replay shared by `tests/{p1,p2,caveats}` (included with `#[path]`): the cases of `<group>/cases.json`
//! run natively against what `cargo xtask pch-gates goldens` recorded from the pinned upstream console scripts,
//! and the Appendix A argparse texts of WP-26's goldens.
//!
//! Owner: WP-27.

use std::ffi::{OsStr, OsString};

use snp_core::env::{Env, EnvPatch};
use snp_hooks::{Epoch, HookCx, NativeSource, Outcome};

use crate::support::{self, Ctx, Sandbox, argv_of, epoch_at, latin1, show};

/// Every supported tag.
pub(crate) const TAGS: [&str; 16] = [
    "v3.0.0", "v3.0.1", "v3.1.0", "v3.2.0", "v3.3.0", "v3.4.0", "v4.0.0", "v4.0.1", "v4.1.0",
    "v4.2.0", "v4.3.0", "v4.4.0", "v4.5.0", "v4.6.0", "v5.0.0", "v6.0.0",
];

/// One native run of a case.
pub(crate) struct Run {
    /// The merged outcome (partitions concatenated).
    pub(crate) outcome: Outcome,
    /// The output with the sandbox paths written as `<root>` / `<home>`.
    pub(crate) output: Vec<u8>,
    /// The bytes of the compared paths after the run.
    pub(crate) after: support::Files,
    /// The compared paths whose bytes changed.
    pub(crate) changed: Vec<String>,
}

/// Applies the steps of a case, including the WP-27 additions `rm` and `home` (and latin-1 `git` arguments).
fn steps(sb: &Sandbox, steps: &serde_json::Value) -> Vec<String> {
    let mut written: Vec<String> = Vec::new();
    for step in steps.as_array().into_iter().flatten() {
        let parts: Vec<&str> = step
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        match parts.as_slice() {
            ["rm", path] => std::fs::remove_file(sb.path(path)).unwrap(),
            // git arguments are latin-1 byte strings too (the driver's `os.fsdecode(b(a))`).
            ["git", args @ ..] => {
                let args: Vec<OsString> = args.iter().map(|a| support::os(&latin1(a))).collect();
                sb.git(&args);
            }
            ["home", path, bytes] => {
                let home = std::path::PathBuf::from(sb.env.get("HOME").unwrap());
                let p = home.join(support::os(&latin1(path)));
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, latin1(bytes)).unwrap();
            }
            _ => {
                for w in sb.steps(&serde_json::Value::Array(vec![step.clone()])) {
                    if !written.contains(&w) {
                        written.push(w);
                    }
                }
            }
        }
    }
    written
}

/// `bytes` with every occurrence of `from` replaced by `to`.
fn replace(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return bytes.to_vec();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// The epoch of `hook` at `tag` when the native reproduces it (`None`: absent or delegated).
pub(crate) fn native_epoch(tag: &str, hook: &str, args: &[OsString]) -> Option<Epoch> {
    let epoch = epoch_at(tag, hook)?;
    let h = snp_hooks::native(NativeSource::PreCommitHooks, hook)?;
    let args: Vec<std::sync::Arc<str>> = args
        .iter()
        .map(|a| std::sync::Arc::from(a.to_string_lossy().as_ref()))
        .collect();
    h.preflight(epoch, &args).is_ok().then_some(epoch)
}

/// Runs one case natively at `tag` (`None` when the native delegates there).
pub(crate) fn run_case(case: &serde_json::Value, tag: &str) -> Option<Run> {
    let hook_id = case["hook"].as_str().unwrap();
    let argv = argv_of(&case["argv"]);
    let epoch = native_epoch(tag, hook_id, &argv)?;
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, hook_id).unwrap();
    if hook.prog() != "pre-commit-hooks-removed" {
        assert_eq!(hook.prog(), case["script"].as_str().unwrap(), "{hook_id}");
    }
    let sb = Sandbox::new(true);
    let written = steps(&sb, &case["steps"]);
    if let Some(big) = case.get("big_then") {
        let mut v = vec![b'a'; usize::try_from(big["fill"].as_u64().unwrap()).unwrap()];
        v.extend_from_slice(&latin1(big["tail"].as_str().unwrap()));
        sb.write(big["path"].as_str().unwrap(), &v);
    }
    let paths: Vec<String> = match case.get("files").and_then(|f| f.as_array()) {
        Some(f) => f.iter().map(|p| p.as_str().unwrap().to_owned()).collect(),
        None => written,
    };
    let before = sb.snapshot(&paths);
    let mut patch = EnvPatch::default();
    if let Some(env) = case.get("env").and_then(|e| e.as_object()) {
        for (k, v) in env {
            patch = patch.set(k.clone(), v.as_str().unwrap());
        }
    }
    let env: Env = sb.env.patched(&patch);
    let root = sb.root.clone();
    let home = std::path::PathBuf::from(sb.env.get("HOME").unwrap());
    let ctx = Ctx::new(sb);
    let cx = HookCx {
        env: &env,
        ..ctx.cx(epoch)
    };
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
    let output = replace(
        &replace(
            &outcome.output,
            home.as_os_str().as_encoded_bytes(),
            b"<home>",
        ),
        root.as_os_str().as_encoded_bytes(),
        b"<root>",
    );
    let after = ctx.sb.snapshot(&paths);
    let changed: Vec<String> = before
        .iter()
        .zip(after.iter())
        .filter(|(b, a)| b.1 != a.1)
        .map(|(b, _)| b.0.clone())
        .collect();
    Some(Run {
        outcome,
        output,
        after,
        changed,
    })
}

/// Lines of `out`, sorted (cases whose Python iterates a `set`).
fn sorted_lines(out: &[u8]) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = out.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    v.sort();
    v
}

/// Replays every case of `group` at every golden tag; returns the number of comparisons.
pub(crate) fn replay(group: &str) -> usize {
    support::link_all();
    let cases = support::json(&format!("{group}/cases.json"));
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    for tag in cases["tags"].as_array().unwrap() {
        let tag = tag.as_str().unwrap();
        let goldens = support::json(&format!("{group}/goldens/{tag}.json"));
        for case in cases["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            if case.get("divergence").is_some() {
                continue;
            }
            let Some(want) = goldens["results"].get(name) else {
                continue;
            };
            let Some(got) = run_case(case, tag) else {
                continue;
            };
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
            if !out_ok
                || got.outcome.code != want_code
                || got.outcome.crashed != want["crashed"].as_bool().unwrap()
            {
                failures.push(format!(
                    "{tag} {name}: code {} (want {want_code}), crashed {} (want {})\n--- got\n{}\n--- want\n{}",
                    got.outcome.code,
                    got.outcome.crashed,
                    want["crashed"],
                    show(&got.output),
                    show(&want_out)
                ));
            }
            for (path, bytes) in &got.after {
                let want_bytes = want["files"].get(path).and_then(|v| v.as_str()).map(latin1);
                if *bytes != want_bytes {
                    failures.push(format!(
                        "{tag} {name}: {path} = {:?} (want {:?})",
                        bytes.as_deref().map(show),
                        want_bytes.as_deref().map(show)
                    ));
                }
            }
            // The natives report exactly the files whose bytes they changed (§3.7).
            let mut modified: Vec<String> = got
                .outcome
                .modified
                .iter()
                .map(|p| support::to_latin1(p.as_bytes()))
                .collect();
            modified.sort();
            modified.dedup();
            let mut changed = got.changed;
            changed.sort();
            if modified != changed {
                failures.push(format!(
                    "{tag} {name}: modified {modified:?} but changed {changed:?}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    checked
}

/// The Appendix A texts (`-h` → stdout, exit 0; an unknown option → usage + error, exit 2) of `scripts` at every
/// supported tag where the native reproduces the hook; returns the number of comparisons. A golden that is a
/// crash (the console script fails before argparse: v4.0.0's missing `main_reverse`, v3.x forbid-new-submodules
/// outside a repository) is skipped: its tag is delegated or the hook takes no options there.
pub(crate) fn appendix_a(scripts: &[(&str, &str)]) -> usize {
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    let ctx = Ctx::new(Sandbox::new(true));
    for tag in TAGS {
        let golden = support::json(&format!("goldens/appendix_a/{tag}.json"));
        for &(hook_id, script) in scripts {
            let Some(want) = golden["scripts"].get(script) else {
                continue;
            };
            let Some(epoch) = native_epoch(tag, hook_id, &[]) else {
                continue;
            };
            let hook = snp_hooks::native(NativeSource::PreCommitHooks, hook_id).unwrap();
            let cx = ctx.cx(epoch);
            for (kind, arg) in [("help", "-h"), ("usage", "--snp-no-such-option")] {
                let w = &want[kind];
                if w["crashed"].as_bool().unwrap_or(false) {
                    continue;
                }
                let got = hook.run(&cx, &[OsStr::new(arg)]);
                checked += 1;
                let want_out = latin1(w["out"].as_str().unwrap());
                if got.output != want_out || i64::from(got.code) != w["code"].as_i64().unwrap() {
                    failures.push(format!(
                        "{tag} {script} {kind}: code {}\n--- got\n{}\n--- want\n{}",
                        got.code,
                        show(&got.output),
                        show(&want_out)
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    checked
}

/// The registry serves each hook under its id and console script, natively at every epoch the data marks
/// native, and refuses epochs outside its range.
pub(crate) fn registry(hooks: &[(&str, &str)]) {
    for &(id, script) in hooks {
        let hook = snp_hooks::native(NativeSource::PreCommitHooks, id).unwrap();
        assert_eq!(hook.id(), id);
        assert!(hook.reports_modifications_exactly());
        let mut native = 0;
        for tag in TAGS {
            if native_epoch(tag, id, &[]).is_some() {
                native += 1;
            }
        }
        assert!(native > 0, "{id} is never native");
        assert!(hook.preflight(Epoch(99), &[]).is_err(), "{id}");
        // The stubs' console script is `pre-commit-hooks-removed`; the real hooks' is the script name.
        if hook.prog() != "pre-commit-hooks-removed" {
            assert_eq!(hook.prog(), script);
        }
        // `repo: builtin` resolves the same native when the builtin manifest lists the id.
        if let Some(b) = snp_hooks::native(NativeSource::Builtin, id) {
            assert_eq!(b.id(), id);
        }
    }
}
