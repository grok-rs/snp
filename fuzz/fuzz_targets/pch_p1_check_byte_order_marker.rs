//! Fuzz target `pch_p1_check_byte_order_marker`: the real (≤ v5.0.0) `check-byte-order-marker` core on arbitrary bytes.
//! The input also drives the native's `run` in a reused sandbox work tree (`run_native`).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p1::kernels::has_bom;

/// The console script's argparse over up to eight NUL-separated argv items taken from `data`.
fn parse(group: &str, script: &str, epoch: u16, data: &[u8]) {
    let argv: Vec<String> = data
        .split(|&b| b == 0)
        .take(8)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    let parsed = match group {
        "p1" => snp_hooks::pch::p1::parse_console_script(script, epoch, &argv, &env),
        "p2" => snp_hooks::pch::p2::parse_console_script(script, epoch, &argv, &env),
        _ => snp_hooks::pch::caveats::parse_console_script(script, epoch, &argv, &env),
    };
    assert!(parsed.is_some());
}

/// A git work tree reused by every iteration of this process (`None` when git is unavailable).
struct Native {
    root: std::path::PathBuf,
    env: snp_core::env::Env,
    git: snp_git::Git,
    facts: snp_git::pch_facts::PchFacts,
    cancel: snp_core::cancel::CancelToken,
}

fn native_sandbox() -> Option<&'static Native> {
    static SANDBOX: std::sync::OnceLock<Option<Native>> = std::sync::OnceLock::new();
    SANDBOX
        .get_or_init(|| {
            let base = std::env::temp_dir().join(format!(
                "snp-fuzz-{}-{}",
                env!("CARGO_BIN_NAME"),
                std::process::id()
            ));
            let (root, home) = (base.join("repo"), base.join("home"));
            std::fs::create_dir_all(&root).ok()?;
            std::fs::create_dir_all(&home).ok()?;
            let process = snp_core::env::Env::capture_process();
            let mut pairs: Vec<(std::ffi::OsString, std::ffi::OsString)> = Vec::new();
            if let Some(path) = process.get("PATH") {
                pairs.push(("PATH".into(), path.to_os_string()));
            }
            pairs.push(("HOME".into(), home.into_os_string()));
            for (k, v) in [
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("LC_ALL", "C.UTF-8"),
            ] {
                pairs.push((k.into(), v.into()));
            }
            let env = snp_core::env::Env::from_pairs(pairs);
            let status = std::process::Command::new("git")
                .args(["init", "-q", "."])
                .current_dir(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .ok()?;
            if !status.success() {
                return None;
            }
            let exe = std::sync::Arc::new(snp_git::GitExe::resolve(&env, &root, None).ok()?);
            let backend: std::sync::Arc<dyn snp_git::GitBackend> =
                std::sync::Arc::new(snp_git::ProcBackend {
                    cancel: snp_core::cancel::CancelToken::new(),
                    role: snp_proc::spawn::Role::Tool,
                });
            let (git, _) = snp_git::Git::discover(exe, &env, &root, backend).ok()?;
            let facts = snp_git::pch_facts::PchFacts::new(git.clone());
            Some(Native {
                root,
                env,
                git,
                facts,
                cancel: snp_core::cancel::CancelToken::new(),
            })
        })
        .as_ref()
}

/// The native `id` itself (`NativeHook::run`: argument handling, file loop, crash ordering) at `epoch`: the first
/// byte selects up to three leading NUL-separated argv items (kept only when they hold no path separator, so
/// writes stay inside the sandbox) and Windows text semantics; the rest is the file `f`, which ends argv.
/// Invariants: no panic; exit 0–2 unless a traceback ended the run; only `f` can be modified.
fn run_native(id: &str, epoch: u16, data: &[u8]) {
    let (Some(n), Some((&sel, mut body))) = (native_sandbox(), data.split_first()) else {
        return;
    };
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    for _ in 0..(sel & 3) {
        let Some(i) = body.iter().position(|&b| b == 0) else {
            break;
        };
        let item = String::from_utf8_lossy(body.get(..i).unwrap_or_default()).into_owned();
        body = body.get(i.saturating_add(1)..).unwrap_or_default();
        if !item.contains(['/', '\\']) && item != ".." && item != "." {
            argv.push(item.into());
        }
    }
    if std::fs::write(n.root.join("f"), body).is_err() {
        return;
    }
    argv.push("f".into());
    let argv_ref: Vec<&OsStr> = argv.iter().map(std::ffi::OsString::as_os_str).collect();
    let Some(hook) = snp_hooks::native(snp_hooks::NativeSource::PreCommitHooks, id) else {
        return;
    };
    let cx = snp_hooks::HookCx {
        root: &n.root,
        env: &n.env,
        git: &n.git,
        color: false,
        epoch: snp_hooks::Epoch(epoch),
        windows: sel & 4 != 0,
        columns: 80,
        facts: &n.facts,
        cancel: &n.cancel,
    };
    let o = hook.run(&cx, &argv_ref);
    assert!(
        o.crashed || (0..=2).contains(&o.code),
        "{id}: exit {}",
        o.code
    );
    for m in &o.modified {
        assert!(
            argv.iter().any(|a| a.as_encoded_bytes() == m.as_bytes()),
            "{id}: modified {m:?}"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    let head = data.get(..3).unwrap_or(data);
    assert_eq!(has_bom(head), data.starts_with(b"\xef\xbb\xbf"));
    parse("p1", "check-byte-order-marker", 1, data);
    run_native("check-byte-order-marker", 1, data);
});
