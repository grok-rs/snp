//! snp-store behaviour beyond the catalog rows (WP-22 acceptance): the location matrix and M7, multi-process
//! materialisation and kill -9, environment-directory naming, install markers, the global lock against gc, sweep,
//! clean, the ephemeral store, markers, relocation and the ownership policy.
//!
//! Multi-process tests re-execute this test binary with `SNP_STORE_CHILD=<role>`; [`child_entry`] is a no-op in a
//! normal run.
//!
//! Owner: WP-22.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use snp_core::brand::Brand;
use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_core::output::CollectWarnings;
use snp_store::{
    MarkerKey, RepoHandle, RepoId, RepoKind, RepoMeta, Store, StoreSource, ToolRef,
    resolve_location,
};

#[cfg(unix)]
use crate::common::is_root;
use crate::common::{exc_text, fake_clone, materialize, new_store, remote, store_at};

const URL: &str = "https://example.invalid/hooks";

// ----- child processes -----

fn child(role: &str, root: &Path, extra: &[(&str, &Path)]) -> Command {
    child_as(&std::env::current_exe().unwrap(), role, root, extra)
}

/// [`child`] started through `exe` (argv[0]).
fn child_as(exe: &Path, role: &str, root: &Path, extra: &[(&str, &Path)]) -> Command {
    let mut c = Command::new(exe);
    c.args([
        "--exact",
        "behaviour::child_entry",
        "--nocapture",
        "--test-threads=1",
    ])
    .env("SNP_STORE_CHILD", role)
    .env("SNP_STORE_ROOT", root)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for (k, v) in extra {
        c.env(k, v);
    }
    c
}

fn wait_for(path: &Path, what: &str) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn finish(c: Child) -> String {
    let out = c.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy_owned(out.stdout);
    assert!(
        out.status.success(),
        "child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// The child half of the multi-process tests (a no-op unless `SNP_STORE_CHILD` is set).
#[test]
fn child_entry() {
    let env = Env::capture_process();
    let Some(role) = env
        .get("SNP_STORE_CHILD")
        .map(|r| r.to_string_lossy().into_owned())
    else {
        return;
    };
    let root = PathBuf::from(env.get("SNP_STORE_ROOT").unwrap());
    let get = |k: &str| PathBuf::from(env.get(k).unwrap());
    let s = store_at(&root);
    s.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    let (key, meta) = remote(URL, "v1", &[]);
    match role.as_str() {
        "materialize" => {
            let counter = get("SNP_STORE_COUNTER");
            let mut fill = |src: &Path| -> Result<(), Failure> {
                std::fs::write(counter.join(std::process::id().to_string()), b"").unwrap();
                std::thread::sleep(Duration::from_millis(300));
                fake_clone(src)
            };
            let h = s
                .materialize(&key, meta, &mut fill, &mut || {}, &CancelToken::new())
                .unwrap();
            println!("SRC {}", h.src.display());
        }
        "fill_forever" => {
            let ready = get("SNP_STORE_READY");
            let mut fill = |src: &Path| -> Result<(), Failure> {
                std::fs::create_dir_all(src.join(".git")).unwrap();
                std::fs::write(src.join("partial"), b"half").unwrap();
                std::fs::write(&ready, b"").unwrap();
                std::thread::sleep(Duration::from_secs(60));
                Ok(())
            };
            let _never = s.materialize(&key, meta, &mut fill, &mut || {}, &CancelToken::new());
        }
        "hold_shared" => {
            let _guard = s.shared(&mut || {}, &CancelToken::new()).unwrap();
            std::fs::write(get("SNP_STORE_READY"), b"").unwrap();
            wait_for(&get("SNP_STORE_RELEASE"), "release");
        }
        "count_waits" => {
            let waits = AtomicUsize::new(0);
            for _ in 0..2 {
                let c = CancelToken::new();
                let c2 = c.clone();
                let t = std::thread::Builder::new()
                    .spawn(move || {
                        std::thread::sleep(Duration::from_millis(200));
                        c2.cancel(CancelReason::Interrupt);
                    })
                    .unwrap();
                let r = s.shared(
                    &mut || {
                        waits.fetch_add(1, Ordering::SeqCst);
                    },
                    &c,
                );
                assert!(matches!(
                    r,
                    Err(Failure::Interrupted(CancelReason::Interrupt))
                ));
                t.join().unwrap();
            }
            println!("WAITS {}", waits.load(Ordering::SeqCst));
        }
        "m7" => {
            let pc = get("SNP_STORE_PC");
            let e = Env::from_pairs([
                ("HOME", root.to_str().unwrap()),
                ("PRE_COMMIT_HOME", pc.to_str().unwrap()),
                ("SNP_HOME", pc.to_str().unwrap()),
            ]);
            let err = resolve_location(&e, &root).unwrap_err();
            println!("M7 {}", exc_text(&err).1);
        }
        other => panic!("unknown child role {other}"),
    }
}

/// Moves `path`'s mtime `by` into the past.
fn age_by(path: &Path, by: Duration) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - by).unwrap();
}

// ----- location -----

fn env(pairs: &[(&str, &str)]) -> Env {
    Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
}

/// Location matrix: `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp`, empty values unset, relative values against
/// the invocation cwd, `HOME=""`, realpath through symlinks, `PRE_COMMIT_HOME` ignored (T10-STO-01 shape).
#[cfg(unix)]
#[test]
fn location_matrix() {
    type Case<'a> = (Vec<(&'a str, &'a str)>, PathBuf, StoreSource);
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    std::fs::create_dir_all(base.join("real/home")).unwrap();
    std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
    let home = base.join("link/home");
    let home_s = home.to_str().unwrap();
    let cwd = base.join("cwd");
    let cases: Vec<Case<'_>> = vec![
        (
            vec![("HOME", home_s)],
            base.join("real/home/.cache/snp"),
            StoreSource::PlatformDefault,
        ),
        (
            vec![("HOME", home_s), ("XDG_CACHE_HOME", "xdg")],
            cwd.join("xdg/snp"),
            StoreSource::XdgCacheHome,
        ),
        (
            vec![
                ("HOME", home_s),
                ("XDG_CACHE_HOME", "/x"),
                ("SNP_HOME", "s"),
            ],
            cwd.join("s"),
            StoreSource::SnpHome,
        ),
        (
            vec![("HOME", home_s), ("SNP_HOME", ""), ("XDG_CACHE_HOME", "")],
            base.join("real/home/.cache/snp"),
            StoreSource::PlatformDefault,
        ),
        (
            vec![("HOME", home_s), ("PRE_COMMIT_HOME", "/elsewhere")],
            base.join("real/home/.cache/snp"),
            StoreSource::PlatformDefault,
        ),
        (
            vec![("HOME", "")],
            PathBuf::from("/.cache/snp"),
            StoreSource::PlatformDefault,
        ),
    ];
    for (pairs, want, source) in cases {
        let loc = resolve_location(&env(&pairs), &cwd).unwrap();
        assert_eq!(loc.path, want, "{pairs:?}");
        assert_eq!(loc.source, source, "{pairs:?}");
    }
    // Nothing was created.
    assert!(!base.join("real/home/.cache").exists());
}

/// Windows: `%USERPROFILE%\.cache\snp` (`HOME` ignored, 14 P-65).
#[cfg(windows)]
#[test]
fn location_matrix_windows() {
    let loc = resolve_location(
        &env(&[("USERPROFILE", r"C:\Users\me"), ("HOME", r"D:\ignored")]),
        Path::new(r"C:\work"),
    )
    .unwrap();
    assert_eq!(loc.path, PathBuf::from(r"C:\Users\me\.cache\snp"));
    assert_eq!(loc.source, StoreSource::PlatformDefault);
}

/// M7: pre-commit's store is refused by path equality and by its README signature; nothing is created.
#[test]
fn m7_refuses_pre_commit_store_by_path_and_by_readme() {
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    let pc = base.join("pc");
    let pc_s = pc.to_str().unwrap();
    let e = env(&[
        ("HOME", base.to_str().unwrap()),
        ("PRE_COMMIT_HOME", pc_s),
        ("SNP_HOME", pc_s),
    ]);
    let err = resolve_location(&e, &base).unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(
        exc_text(&err),
        (
            "FatalError".to_owned(),
            format!(
                "snp refuses to use pre-commit's cache directory {}; set SNP_HOME to another directory",
                pc.display()
            )
        )
    );
    assert!(!pc.exists());
    // A copy of pre-commit's store elsewhere, recognised by its README.
    let copy = base.join("copy");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(
        copy.join("README"),
        b"This directory is maintained by the pre-commit project.\nLearn more: https://github.com/pre-commit/pre-commit\n",
    )
    .unwrap();
    let e = env(&[
        ("HOME", base.to_str().unwrap()),
        ("SNP_HOME", copy.to_str().unwrap()),
    ]);
    assert_eq!(resolve_location(&e, &base).unwrap_err().exit_code(), 1);
    // The default pre-commit store (`~/.cache/pre-commit`) too (`~` is USERPROFILE on Windows).
    let e = env(&[
        ("HOME", base.to_str().unwrap()),
        ("USERPROFILE", base.to_str().unwrap()),
        ("SNP_HOME", base.join(".cache/pre-commit").to_str().unwrap()),
    ]);
    assert_eq!(resolve_location(&e, &base).unwrap_err().exit_code(), 1);
    // snp's own README is fine.
    let own = base.join("own");
    let s = store_at(&own);
    s.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    let e = env(&[
        ("HOME", base.to_str().unwrap()),
        ("SNP_HOME", own.to_str().unwrap()),
    ]);
    assert_eq!(resolve_location(&e, &base).unwrap().path, own);
}

/// M7 names the program by brand: run as `pre-commit` (argv[0], ADR-023) the text starts with `pre-commit refuses`.
#[cfg(unix)]
#[test]
fn m7_names_the_program_by_brand() {
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    let pc = base.join("pc");
    let link = base.join("pre-commit");
    std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
    for (exe, prog) in [
        (link, "pre-commit"),
        (std::env::current_exe().unwrap(), "snp"),
    ] {
        // `Command::new(path)` passes the path as given as argv[0] (the symlink, not its target).
        let mut c = child_as(&exe, "m7", &base, &[("SNP_STORE_PC", &pc)]);
        let out = finish(c.spawn().unwrap_or_else(|e| panic!("{exe:?}: {e}")));
        assert!(
            out.contains(&format!(
                "M7 {prog} refuses to use pre-commit's cache directory {}; set SNP_HOME to another directory",
                pc.display()
            )),
            "{out}"
        );
    }
}

// ----- materialisation -----

/// 8 concurrent processes materialise one key: exactly one fill, all succeed with the same entry.
#[test]
fn eight_processes_materialize_one_key_with_one_fill() {
    let (_g, root, _s) = new_store();
    let counter = root.parent().unwrap().join("fills");
    std::fs::create_dir_all(&counter).unwrap();
    #[expect(
        clippy::needless_collect,
        reason = "all children must start before the first is awaited"
    )]
    let children: Vec<Child> = (0..8)
        .map(|_| {
            child("materialize", &root, &[("SNP_STORE_COUNTER", &counter)])
                .spawn()
                .unwrap()
        })
        .collect();
    let srcs: Vec<String> = children
        .into_iter()
        .map(|c| {
            finish(c)
                .lines()
                .find_map(|l| l.split_once("SRC ").map(|(_, p)| p.to_owned()))
                .unwrap()
        })
        .collect();
    assert_eq!(
        std::fs::read_dir(&counter).unwrap().count(),
        1,
        "exactly one fill"
    );
    assert!(srcs.iter().all(|s| *s == srcs[0]), "{srcs:?}");
    let (key, _) = remote(URL, "v1", &[]);
    let h = store_at(&root).lookup(&key).unwrap();
    assert_eq!(h.src.display().to_string(), srcs[0]);
}

/// kill -9 during the fill: the next run refills and never uses the partial entry; sweep collects the orphan.
#[test]
fn kill_9_during_fill_leaves_no_partial_entry() {
    let (_g, root, s) = new_store();
    let ready = root.parent().unwrap().join("ready");
    let mut c = child("fill_forever", &root, &[("SNP_STORE_READY", &ready)])
        .spawn()
        .unwrap();
    wait_for(&ready, "the child's fill");
    c.kill().unwrap();
    let _status = c.wait().unwrap();
    let (key, meta) = remote(URL, "v1", &[]);
    assert!(s.lookup(&key).is_none(), "a partial entry is never visible");
    assert_eq!(std::fs::read_dir(s.scratch_dir()).unwrap().count(), 1);
    let h = materialize(&s, &key, meta);
    assert!(h.src.join(".pre-commit-hooks.yaml").is_file());
    assert!(!h.src.join("partial").exists());
    // The orphan scratch dir goes once it is older than an hour.
    assert_eq!(s.sweep(SystemTime::now()).removed_dirs, 0);
    let report = s.sweep(SystemTime::now() + Duration::from_hours(2));
    assert_eq!(report.removed_dirs, 1);
    assert_eq!(std::fs::read_dir(s.scratch_dir()).unwrap().count(), 0);
    assert!(s.lookup(&key).is_some());
}

/// A materialised entry is found again without filling (T10-STO-06 shape) and a failing fill leaves nothing.
#[test]
fn materialize_is_double_checked_and_failures_leave_nothing() {
    let (_g, _root, s) = new_store();
    let (key, meta) = remote(URL, "v2", &["dep"]);
    let mut fails = |_: &Path| -> Result<(), Failure> {
        Err(Failure::unexpected("CalledProcessError", "clone failed"))
    };
    let err = s
        .materialize(
            &key,
            meta.clone(),
            &mut fails,
            &mut || {},
            &CancelToken::new(),
        )
        .unwrap_err();
    assert_eq!(exc_text(&err).1, "clone failed");
    assert!(s.lookup(&key).is_none());
    assert_eq!(std::fs::read_dir(s.scratch_dir()).unwrap().count(), 0);
    let h = materialize(&s, &key, meta.clone());
    let mut never = |_: &Path| -> Result<(), Failure> { panic!("must not fill again") };
    let again = s
        .materialize(&key, meta, &mut never, &mut || {}, &CancelToken::new())
        .unwrap();
    assert_eq!(again.src, h.src);
    // A cancelled token before the fill.
    let (key2, meta2) = remote(URL, "v3", &[]);
    let c = CancelToken::new();
    c.cancel(CancelReason::Terminate);
    let err = s
        .materialize(&key2, meta2, &mut fake_clone, &mut || {}, &c)
        .unwrap_err();
    assert!(matches!(err, Failure::Interrupted(CancelReason::Terminate)));
    // Metadata that does not derive the key is a bug, never stored.
    let (key3, _) = remote(URL, "v4", &[]);
    let (_, meta4) = remote(URL, "v5", &[]);
    assert!(
        s.materialize(
            &key3,
            meta4,
            &mut fake_clone,
            &mut || {},
            &CancelToken::new()
        )
        .is_err()
    );
    assert_eq!(s.all_repos().len(), 1);
}

/// Local template keys include the template version and the dependencies; entries are listed by key.
#[test]
fn local_keys_and_all_repos() {
    let (_g, _root, s) = new_store();
    let deps: Arc<[Arc<str>]> = Arc::from([Arc::from("six")]);
    let key = Store::local_key(&deps);
    assert_ne!(key, Store::local_key(&[]));
    let meta = RepoMeta {
        kind: RepoKind::Local,
        deps,
    };
    let mut fill = |src: &Path| -> Result<(), Failure> {
        std::fs::write(src.join("setup.py"), b"x").unwrap();
        Ok(())
    };
    let h = s
        .materialize(&key, meta, &mut fill, &mut || {}, &CancelToken::new())
        .unwrap();
    assert!(matches!(h.meta.kind, RepoKind::Local));
    let (k2, m2) = remote(URL, "v1", &[]);
    materialize(&s, &k2, m2);
    let all: Vec<_> = s.all_repos().into_iter().map(|h| h.key).collect();
    let mut want = vec![key, k2];
    want.sort_by_key(|k| k.0);
    assert_eq!(all, want);
    s.retire_repo(&key).unwrap();
    s.retire_repo(&key).unwrap(); // ENOENT-tolerant (X13-CMD-07)
    assert_eq!(s.all_repos().len(), 1);
}

// ----- environments and install markers -----

fn handle(s: &Store) -> RepoHandle {
    let (key, meta) = remote(URL, "env", &[]);
    materialize(s, &key, meta)
}

/// Env-dir naming: safe versions are literal; `../../x`, `/abs`, `C:\x`, NUL, empty, `.`/`..` and > 64 chars are
/// hashed, stay one component inside `envs`, and a contained removal never deletes anything outside it.
#[test]
fn env_dir_names_are_sanitized_or_hashed() {
    let (_g, root, s) = new_store();
    let h = handle(&s);
    std::fs::create_dir_all(&h.envs).unwrap();
    for v in [
        "python3.12",
        "3.12.1",
        "default",
        "system",
        "22.11.0",
        "1.0+local",
    ] {
        let d = s.env_dir(&h, LanguageId::Python, v);
        assert_eq!(
            d.file_name().unwrap().to_str().unwrap(),
            format!("py_env-{v}")
        );
    }
    assert!(
        s.env_dir(&h, LanguageId::Node, "system")
            .ends_with("node_env-system")
    );
    let sentinel = root.join("sentinel");
    std::fs::create_dir_all(&sentinel).unwrap();
    std::fs::write(sentinel.join("keep"), b"x").unwrap();
    let long: String = std::iter::repeat_n('9', 65).collect();
    for v in [
        "../../x",
        "/abs",
        "C:\\x",
        "a\0b",
        "",
        ".",
        "..",
        "a/b",
        long.as_str(),
        "../../../sentinel",
        // Win32 strips trailing dots: `py_env-3.12.` would open `py_env-3.12`.
        "3.12.",
        "...",
    ] {
        let d = s.env_dir(&h, LanguageId::Python, v);
        assert_eq!(d.parent().unwrap(), h.envs, "{v:?}");
        let name = d.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(
            name.starts_with("py_env-h") && name.len() == "py_env-h".len() + 16,
            "{name}"
        );
        std::fs::create_dir_all(d.join("bin")).unwrap();
        snp_os::fs::remove_tree_within(&d, &h.envs).unwrap();
        assert!(!d.exists());
    }
    assert_ne!(
        s.env_dir(&h, LanguageId::Python, "../../x"),
        s.env_dir(&h, LanguageId::Python, "/abs")
    );
    // Versions differing only in case never share a directory on a case-folding filesystem.
    let upper = s.env_dir(&h, LanguageId::Python, "3.12A");
    let lower = s.env_dir(&h, LanguageId::Python, "3.12a");
    assert!(lower.ends_with("py_env-3.12a"));
    assert_ne!(upper, lower);
    if cfg!(any(windows, target_vendor = "apple")) {
        assert_ne!(
            upper.file_name().unwrap().to_ascii_lowercase(),
            lower.file_name().unwrap().to_ascii_lowercase()
        );
    } else {
        assert!(upper.ends_with("py_env-3.12A"), "case-sensitive: verbatim");
    }
    assert!(sentinel.join("keep").exists());
    assert!(h.src.join(".pre-commit-hooks.yaml").exists());
}

/// Install markers: round trip; the tool sidecar is written; malformed or missing state means "reinstall".
#[test]
fn install_marker_round_trip_and_malformed_state() {
    let (_g, _root, s) = new_store();
    let h = handle(&s);
    let env_dir = s.env_dir(&h, LanguageId::Node, "22.11.0");
    std::fs::create_dir_all(&env_dir).unwrap();
    assert_eq!(Store::read_install_marker(&env_dir), None);
    let json = br#"{"v":1,"language":"node","additional_dependencies":[]}"#;
    let tools = [ToolRef {
        tool: Arc::from("node"),
        version: Arc::from("22.11.0"),
    }];
    Store::commit_install_marker(&env_dir, json, &tools).unwrap();
    assert_eq!(
        Store::read_install_marker(&env_dir).as_deref(),
        Some(&json[..])
    );
    let sidecar = std::fs::read_to_string(env_dir.join(".snp-tools.json")).unwrap();
    assert!(
        sidecar.contains("\"node\"") && sidecar.contains("22.11.0"),
        "{sidecar}"
    );
    for bad in [&b"{bad"[..], b"", b"\xff\xfe", br#"{"v":1,"#] {
        std::fs::write(env_dir.join(".snp-install-state.json"), bad).unwrap();
        assert_eq!(Store::read_install_marker(&env_dir), None, "{bad:?}");
    }
    // Re-commit over a malformed state.
    Store::commit_install_marker(&env_dir, json, &[]).unwrap();
    assert!(Store::read_install_marker(&env_dir).is_some());
}

// ----- locks, gc, sweep, clean -----

/// gc (exclusive) can never remove a repository while another process holds the shared lock.
#[test]
fn exclusive_waits_for_another_process_holding_the_shared_lock() {
    let (_g, root, store) = new_store();
    let (key, meta) = remote(URL, "held", &[]);
    materialize(&store, &key, meta);
    let ready = root.parent().unwrap().join("held-ready");
    let release = root.parent().unwrap().join("held-release");
    let holder = child(
        "hold_shared",
        &root,
        &[("SNP_STORE_READY", &ready), ("SNP_STORE_RELEASE", &release)],
    )
    .spawn()
    .unwrap();
    wait_for(&ready, "the shared lock");
    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    let canceller = std::thread::Builder::new()
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel2.cancel(CancelReason::Interrupt);
        })
        .unwrap();
    let err = store.exclusive(&mut || {}, &cancel).unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(err, Failure::Interrupted(CancelReason::Interrupt)));
    assert!(store.lookup(&key).is_some(), "nothing was removed");
    std::fs::write(&release, b"").unwrap();
    finish(holder);
    let guard = store.exclusive(&mut || {}, &CancelToken::new()).unwrap();
    store.retire_repo(&key).unwrap();
    drop(guard);
    assert!(store.lookup(&key).is_none());
    // Shared locks coexist.
    let a = store.shared(&mut || {}, &CancelToken::new()).unwrap();
    let b = store.shared(&mut || {}, &CancelToken::new()).unwrap();
    drop((a, b));
}

/// The contention callback (`[INFO] Locking … directory`) runs at most once per process.
#[test]
fn lock_wait_callback_runs_once_per_process() {
    let (_g, root, s) = new_store();
    let guard = s.exclusive(&mut || {}, &CancelToken::new()).unwrap();
    let out = finish(child("count_waits", &root, &[]).spawn().unwrap());
    drop(guard);
    assert!(out.contains("WAITS 1"), "{out}");
    assert_eq!(
        snp_store::locking_message(Brand::PreCommit),
        "Locking pre-commit directory"
    );
    assert_eq!(
        snp_store::locking_message(Brand::Snp),
        "Locking snp directory"
    );
}

/// Per-repository and per-tool locks are exclusive and independent.
#[test]
fn repo_and_tool_locks() {
    let (_g, root, s) = new_store();
    let (key, _) = remote(URL, "lock", &[]);
    let l = s.repo_lock(&key, &mut || {}, &CancelToken::new()).unwrap();
    let path = root.join("repos-v1").join(format!("{}.lock", key.hex()));
    assert!(
        snp_os::lock::FileLock::try_acquire(&path, snp_os::lock::LockMode::Exclusive)
            .unwrap()
            .is_none()
    );
    let t = s
        .tool_lock("node", &mut || {}, &CancelToken::new())
        .unwrap();
    assert!(root.join("tools-v1/node.lock").exists());
    let odd = s
        .tool_lock("../evil", &mut || {}, &CancelToken::new())
        .unwrap();
    assert!(!root.join("evil.lock").exists());
    drop((l, t, odd));
    // Released (a child spawned concurrently by another test may hold a forked copy of the fd until its exec).
    let start = Instant::now();
    while snp_os::lock::FileLock::try_acquire(&path, snp_os::lock::LockMode::Exclusive)
        .unwrap()
        .is_none()
    {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the repo lock was not released"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// sweep: restored patches older than 30 days go, unrestored ones (mentioned by a journal) and journals stay;
/// tools go only when no `.snp-tools.json` references them, they are older than an hour and their lock is free.
#[test]
fn sweep_keeps_unrestored_patches_and_referenced_tools() {
    let (_g, root, s) = new_store();
    let patches = s.patches_dir();
    std::fs::write(patches.join("patch1700000000-41"), b"restored").unwrap();
    std::fs::write(patches.join("patch1700000000-42"), b"unrestored").unwrap();
    std::fs::write(
        patches.join("patch1700000000-43"),
        b"restored, crash before cleanup",
    )
    .unwrap();
    std::fs::write(
        patches.join("run-43-abcd.snp-journal"),
        // a JSON string (a Windows path's `\` is escaped)
        format!(
            r#"{{"v":1,"state":"restored","patch":{}}}"#,
            serde_json::to_string(&patches.join("patch1700000000-43")).unwrap()
        ),
    )
    .unwrap();
    let journal = patches.join("run-42-abcd.snp-journal");
    std::fs::write(
        &journal,
        format!(
            r#"{{"v":1,"state":"failed","patch":{}}}"#,
            serde_json::to_string(&patches.join("patch1700000000-42")).unwrap()
        ),
    )
    .unwrap();
    // Tools: node referenced, go unreferenced, uv unreferenced but locked.
    for (t, v) in [("node", "22.11.0"), ("go", "1.21.0"), ("uv", "0.5.0")] {
        std::fs::create_dir_all(s.tool_dir(t, v).join("bin")).unwrap();
    }
    let h = handle(&s);
    let env_dir = s.env_dir(&h, LanguageId::Node, "22.11.0");
    std::fs::create_dir_all(&env_dir).unwrap();
    Store::commit_install_marker(
        &env_dir,
        b"{}",
        &[ToolRef {
            tool: Arc::from("node"),
            version: Arc::from("22.11.0"),
        }],
    )
    .unwrap();
    let uv_lock = s.tool_lock("uv", &mut || {}, &CancelToken::new()).unwrap();

    let now = SystemTime::now();
    let r = s.sweep(now);
    assert_eq!((r.removed_dirs, r.removed_patches), (0, 0));
    let r = s.sweep(now + Duration::from_hours(2));
    assert_eq!(r.removed_dirs, 1, "only go");
    assert!(!s.tool_dir("go", "1.21.0").exists());
    assert!(s.tool_dir("node", "22.11.0").exists());
    assert!(s.tool_dir("uv", "0.5.0").exists());
    drop(uv_lock);
    let r = s.sweep(now + Duration::from_hours(31 * 24));
    assert_eq!(r.removed_patches, 2);
    assert!(!patches.join("patch1700000000-41").exists());
    assert!(
        !patches.join("patch1700000000-43").exists(),
        "restored: 30-day rule"
    );
    assert!(patches.join("run-43-abcd.snp-journal").exists());
    assert!(patches.join("patch1700000000-42").exists());
    assert!(journal.exists());
    assert!(!s.tool_dir("uv", "0.5.0").exists());
    assert!(s.tool_dir("node", "22.11.0").exists());
    assert!(!r.legacy_v1_removed);
    // A malformed sidecar: nothing is known, nothing is removed.
    std::fs::create_dir_all(s.tool_dir("deno", "2.0.0")).unwrap();
    std::fs::write(env_dir.join(".snp-tools.json"), b"{garbage").unwrap();
    let _report = s.sweep(now + Duration::from_hours(40 * 24));
    assert!(s.tool_dir("deno", "2.0.0").exists());
    assert!(root.join("tools-v1").exists());
}

/// sweep removes buckets of other format versions, orphan repo locks and snp 1.x leftovers behind the 1.x README.
#[test]
fn sweep_removes_other_versions_and_snp1_leftovers() {
    let (_g, root, s) = new_store();
    std::fs::create_dir_all(root.join("repos-v0/x")).unwrap();
    std::fs::create_dir_all(root.join("repos")).unwrap();
    std::fs::write(root.join("db.db"), b"sqlite").unwrap();
    let (key, _) = remote(URL, "gone", &[]);
    drop(s.repo_lock(&key, &mut || {}, &CancelToken::new()).unwrap());
    let r = s.sweep(SystemTime::now());
    assert_eq!(r.removed_dirs, 1);
    assert!(!root.join("repos-v0").exists());
    assert!(
        !root
            .join("repos-v1")
            .join(format!("{}.lock", key.hex()))
            .exists()
    );
    // Without the 1.x signature, 1.x-looking names are not ours to delete.
    assert!(root.join("repos").exists() && root.join("db.db").exists());
    assert!(!r.legacy_v1_removed);
    std::fs::write(
        root.join("README"),
        b"This directory is maintained by the SNP (Shell Not Pass) project.\nLearn more: https://github.com/devops247-online/snp\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("environments/python-3.12-abc")).unwrap();
    let r = s.sweep(SystemTime::now());
    assert!(r.legacy_v1_removed);
    for gone in ["repos", "environments", "db.db"] {
        assert!(!root.join(gone).exists(), "{gone}");
    }
    assert!(
        std::fs::read_to_string(root.join("README"))
            .unwrap()
            .starts_with("This directory is maintained by snp (You Shell Not Pass).")
    );
    assert!(root.join("repos-v1").exists());
}

/// `clean` on a missing store is Ok and creates nothing.
#[test]
fn clean_on_a_missing_store_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("never");
    let s = store_at(&root);
    s.clean().unwrap();
    assert!(!root.exists());
    assert!(!s.exists());
}

/// `clean` retires snp's layout; unknown files, unrestored stash patches and journals survive.
#[test]
fn clean_retires_the_layout_but_keeps_unknown_and_recovery_data() {
    let (_g, root, s) = new_store();
    let (key, meta) = remote(URL, "c", &[]);
    materialize(&s, &key, meta);
    s.set_marker(&MarkerKey::Initialized {
        db_repo_name: Arc::from(URL),
        rev: Arc::from("c"),
    });
    std::fs::write(root.join("user-notes.txt"), b"mine").unwrap();
    std::fs::write(root.join("snp.log"), b"crash").unwrap();
    let patches = s.patches_dir();
    std::fs::write(patches.join("patch1-1"), b"restored").unwrap();
    std::fs::write(patches.join("patch2-2"), b"kept").unwrap();
    std::fs::write(patches.join("patch3-3"), b"restored, journal left behind").unwrap();
    std::fs::write(
        patches.join("run-2-x.snp-journal"),
        b"{\"state\":\"failed\",\"patch\":\"/p/patch2-2\"}",
    )
    .unwrap();
    std::fs::write(
        patches.join("run-3-x.snp-journal"),
        b"{\"state\":\"restored\",\"patch\":\"/p/patch3-3\"}",
    )
    .unwrap();
    for p in ["patch1-1", "patch2-2", "patch3-3"] {
        age_by(&patches.join(p), Duration::from_hours(2));
    }
    // A run in another worktree just wrote its patch and has not journaled it yet.
    std::fs::write(patches.join("patch4-4"), b"stashing right now").unwrap();
    s.clean().unwrap();
    for gone in [
        "repos-v1",
        "keys-v1",
        "configs-v1",
        "tools-v1",
        "cache-v1",
        "scratch",
        "trash",
        "spill",
        "README",
        "store-v1.json",
        "snp.log",
    ] {
        assert!(!root.join(gone).exists(), "{gone}");
    }
    assert!(root.join("user-notes.txt").exists());
    assert!(!patches.join("patch1-1").exists());
    assert!(patches.join("patch2-2").exists());
    assert!(
        !patches.join("patch3-3").exists(),
        "a restored journal protects nothing"
    );
    assert!(
        patches.join("patch4-4").exists(),
        "a fresh unjournaled patch is kept"
    );
    assert!(patches.join("run-2-x.snp-journal").exists());
    assert!(patches.join("run-3-x.snp-journal").exists());
    // The store comes back on demand.
    s.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    let s2 = store_at(&root);
    s2.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert!(root.join("repos-v1").is_dir());
    assert!(s2.lookup(&key).is_none());
}

/// A journal as `snp-stash` writes it (`JournalData::to_bytes`: `ita` before `patch`), `patch` given as JSON.
fn journal_json(state: &str, ita: &[String], patch: &str) -> String {
    format!(
        r#"{{"v":1,"state":"{state}","ita":{},"patch":{patch}}}"#,
        serde_json::to_string(ita).unwrap()
    )
}

/// `patch` as `snp-stash` journals a UTF-8 path: a JSON string.
fn json_path(p: &Path) -> String {
    serde_json::to_string(p.to_str().unwrap()).unwrap()
}

/// F1 (D-DATA-04): a stash journal gc cannot read or parse protects every patch (fail closed), for `sweep` and
/// `clean` alike; an unreadable journal is a directory here (portable, and root cannot read it either).
#[test]
fn sweep_and_clean_keep_every_patch_when_a_journal_is_unreadable() {
    for broken in ["dir", "garbage"] {
        let (_g, _root, s) = new_store();
        let patches = s.patches_dir();
        let journal = patches.join("run-9-abcd.snp-journal");
        match broken {
            "dir" => std::fs::create_dir_all(&journal).unwrap(),
            _ => std::fs::write(&journal, br#"{"v":1,"state":"stashed","patch":"/s/patc"#).unwrap(),
        }
        std::fs::write(patches.join("patch1700000000-9"), b"maybe the only copy").unwrap();
        let r = s.sweep(SystemTime::now() + Duration::from_hours(31 * 24));
        assert_eq!(r.removed_patches, 0, "{broken}");
        assert_eq!(r.unreadable_journal.as_deref(), Some(&*journal), "{broken}");
        assert!(patches.join("patch1700000000-9").exists(), "{broken}");
        age_by(&patches.join("patch1700000000-9"), Duration::from_hours(2));
        assert_eq!(s.clean().unwrap().as_deref(), Some(&*journal), "{broken}");
        assert!(patches.join("patch1700000000-9").exists(), "{broken}");
        assert!(journal.exists(), "{broken}");
    }
}

/// F1 (D-DATA-04): a journal of any size protects its patch: a long intent-to-add list puts `patch` past 1 MiB.
#[test]
fn sweep_protects_the_patch_of_a_journal_over_one_mebibyte() {
    let (_g, _root, s) = new_store();
    let patches = s.patches_dir();
    let ita: Vec<String> = (0..40_000)
        .map(|i| format!("some/deep/dir/file-{i:08}.txt"))
        .collect();
    let body = journal_json(
        "failed",
        &ita,
        &json_path(&patches.join("patch1700000000-8")),
    );
    assert!(body.len() > 1024 * 1024);
    std::fs::write(patches.join("run-8-abcd.snp-journal"), body).unwrap();
    std::fs::write(patches.join("patch1700000000-8"), b"the only copy").unwrap();
    std::fs::write(patches.join("patch1700000000-7"), b"restored long ago").unwrap();
    let r = s.sweep(SystemTime::now() + Duration::from_hours(31 * 24));
    assert!(patches.join("patch1700000000-8").exists());
    assert!(!patches.join("patch1700000000-7").exists());
    assert_eq!((r.removed_patches, r.unreadable_journal), (1, None));
}

/// F1 (D-DATA-04): a store at a non-UTF-8 path journals its patch as `{"hex": …}` (`snp-stash` `enc_bytes`); gc
/// decodes it like the stash does.
#[cfg(unix)]
#[test]
fn sweep_and_clean_protect_a_patch_journaled_as_hex() {
    use std::os::unix::ffi::OsStrExt;
    let (_g, _root, s) = new_store();
    let patches = s.patches_dir();
    let mut named = b"/s/\xffstore/patches/".to_vec();
    named.extend_from_slice(b"patch1700000000-6");
    let hex = named.iter().fold(String::new(), |mut h, b| {
        std::fmt::Write::write_fmt(&mut h, format_args!("{b:02x}")).unwrap();
        h
    });
    std::fs::write(
        patches.join("run-6-abcd.snp-journal"),
        journal_json("failed", &[], &format!(r#"{{"hex":"{hex}"}}"#)),
    )
    .unwrap();
    std::fs::write(patches.join("patch1700000000-6"), b"the only copy").unwrap();
    std::fs::write(patches.join("patch1700000000-5"), b"restored long ago").unwrap();
    assert!(std::ffi::OsStr::from_bytes(&named).to_str().is_none());
    let r = s.sweep(SystemTime::now() + Duration::from_hours(31 * 24));
    assert!(patches.join("patch1700000000-6").exists());
    assert!(!patches.join("patch1700000000-5").exists());
    assert_eq!((r.removed_patches, r.unreadable_journal), (1, None));
    age_by(&patches.join("patch1700000000-6"), Duration::from_hours(2));
    assert_eq!(s.clean().unwrap(), None);
    assert!(patches.join("patch1700000000-6").exists());
}

/// F1 (D-DATA-04): the `<patch>.*` siblings of an unrestored patch (the hint's `<patch>.now` backup, the kept
/// `.ita` and `.paths` lists) are protected with it; those of a restored patch, and other patches, are not.
#[test]
fn sweep_and_clean_protect_the_siblings_of_an_unrestored_patch() {
    let (_g, _root, s) = new_store();
    let patches = s.patches_dir();
    std::fs::write(
        patches.join("run-4-abcd.snp-journal"),
        journal_json(
            "failed",
            &[],
            &json_path(&patches.join("patch1700000000-4")),
        ),
    )
    .unwrap();
    std::fs::write(
        patches.join("run-3-abcd.snp-journal"),
        journal_json(
            "restored",
            &[],
            &json_path(&patches.join("patch1700000000-3")),
        ),
    )
    .unwrap();
    let kept = [
        "patch1700000000-4",
        "patch1700000000-4.now",
        "patch1700000000-4.ita",
        "patch1700000000-4.paths",
    ];
    let gone = [
        "patch1700000000-44",
        "patch1700000000-4-1",
        "patch1700000000-3",
        "patch1700000000-3.now",
    ];
    for n in kept.iter().chain(&gone) {
        std::fs::write(patches.join(n), n.as_bytes()).unwrap();
    }
    let r = s.sweep(SystemTime::now() + Duration::from_hours(31 * 24));
    for n in kept {
        assert!(patches.join(n).exists(), "{n}");
    }
    for n in gone {
        assert!(!patches.join(n).exists(), "{n}");
    }
    assert_eq!(r.removed_patches, gone.len());
    for n in [&kept[..], &gone[..]].concat() {
        std::fs::write(patches.join(n), n.as_bytes()).unwrap();
        age_by(&patches.join(n), Duration::from_hours(2));
    }
    assert_eq!(s.clean().unwrap(), None);
    for n in kept {
        assert!(patches.join(n).exists(), "clean: {n}");
    }
    for n in gone {
        assert!(!patches.join(n).exists(), "clean: {n}");
    }
}

// ----- ephemeral store, markers, configs, relocation -----

/// try-repo's store: repos/keys/configs/patches under its root, tools and caches from the shared store.
#[test]
fn ephemeral_store_keeps_repos_inside_and_reads_tools_from_the_shared_store() {
    let (_g, main_root, main) = new_store();
    let tmp = tempfile::tempdir().unwrap();
    let eroot = tmp.path().join("try-repo");
    std::fs::create_dir_all(&eroot).unwrap();
    let e = Store::ephemeral(&eroot, Some(&main));
    e.ensure_created(Brand::PreCommit, &CollectWarnings::new())
        .unwrap();
    let (key, meta) = remote(URL, "try", &[]);
    let h = materialize(&e, &key, meta);
    assert!(h.src.starts_with(&eroot));
    assert!(
        main.lookup(&key).is_none(),
        "the main store gains no repos-v1 entry"
    );
    let m = MarkerKey::Initialized {
        db_repo_name: Arc::from(URL),
        rev: Arc::from("try"),
    };
    e.set_marker(&m);
    assert!(e.marker_exists(&m) && !main.marker_exists(&m));
    assert_eq!(e.tool_dir("node", "22"), main.tool_dir("node", "22"));
    assert_eq!(e.cache_dir("uv"), main.cache_dir("uv"));
    assert!(e.patches_dir().starts_with(&eroot));
    assert!(e.scratch_dir().starts_with(&eroot));
    assert!(!eroot.join("README").exists());
    assert!(!eroot.join("tools-v1").exists());
    // Its shared lock also holds the main store's shared lock (gc there cannot delete the tools in use).
    let guard = e.shared(&mut || {}, &CancelToken::new()).unwrap();
    let c = CancelToken::new();
    c.cancel(CancelReason::Interrupt);
    assert!(main.exclusive(&mut || {}, &c).is_err());
    drop(guard);
    assert!(main.exclusive(&mut || {}, &CancelToken::new()).is_ok());
    // Without a shared store, everything lives under the root.
    let alone = Store::ephemeral(&tmp.path().join("alone"), None);
    assert!(
        alone
            .tool_dir("go", "1")
            .starts_with(tmp.path().join("alone"))
    );
    assert!(main_root.join("tools-v1").exists());
}

/// An ephemeral store over a main store that does not exist yet: `ensure_created` creates the main store with its
/// README, `store-v1.json` and layout (its ownership policy applied), and the shared lock covers it.
#[test]
fn ephemeral_store_creates_its_main_store_through_ensure_created() {
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    let main = store_at(&base.join("main"));
    let e = Store::ephemeral(&base.join("eph"), Some(&main));
    // Locks never create a store implicitly.
    let err = e
        .tool_lock("node", &mut || {}, &CancelToken::new())
        .unwrap_err();
    assert_eq!(exc_text(&err).0, "FileNotFoundError");
    let (key, _) = remote(URL, "x", &[]);
    assert!(e.repo_lock(&key, &mut || {}, &CancelToken::new()).is_err());
    assert!(!base.join("main").exists() && !base.join("eph").exists());

    e.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    let guard = e.shared(&mut || {}, &CancelToken::new()).unwrap();
    let lock = e
        .tool_lock("node", &mut || {}, &CancelToken::new())
        .unwrap();
    let main_root = base.join("main");
    assert!(main_root.join("README").is_file(), "README");
    assert!(main_root.join("store-v1.json").is_file(), "store-v1.json");
    assert!(main_root.join("tools-v1/node.lock").exists());
    assert!(!base.join("eph/README").exists());
    // gc on the main store is excluded while the ephemeral store is in use.
    let c = CancelToken::new();
    c.cancel(CancelReason::Interrupt);
    assert!(main.exclusive(&mut || {}, &c).is_err());
    drop((guard, lock));
    // The main store is a regular store: a fresh handle needs nothing more.
    let again = store_at(&main_root);
    again
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(again.relocated_from(), None);
}

/// A store that never ran `ensure_created` is not created by a repository lock or a materialisation.
#[test]
fn locks_never_create_the_store_root() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("lazy");
    let s = store_at(&root);
    let (key, meta) = remote(URL, "lazy", &[]);
    assert!(s.repo_lock(&key, &mut || {}, &CancelToken::new()).is_err());
    assert!(
        s.materialize(&key, meta, &mut fake_clone, &mut || {}, &CancelToken::new())
            .is_err()
    );
    assert!(s.tool_lock("go", &mut || {}, &CancelToken::new()).is_err());
    assert!(!root.exists());
}

/// A transient error reading an entry (EACCES here) fails the materialisation and leaves the entry alone: another
/// process may be running hooks from it. Only a definitively broken entry is replaced.
#[cfg(unix)]
#[test]
fn transient_entry_errors_never_retire_the_entry() {
    use std::os::unix::fs::PermissionsExt;
    if is_root() {
        return;
    }
    let (_g, root, s) = new_store();
    let (key, meta) = remote(URL, "busy", &[]);
    let h = materialize(&s, &key, meta.clone());
    let dir = root.join("repos-v1").join(key.hex());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut fills = 0;
    let r = s.materialize(
        &key,
        meta.clone(),
        &mut |src| {
            fills += 1;
            fake_clone(src)
        },
        &mut || {},
        &CancelToken::new(),
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let err = r.unwrap_err();
    assert_eq!(exc_text(&err).0, "PermissionError");
    assert_eq!(fills, 0);
    assert!(h.src.join(".git").exists(), "the entry in use survives");
    assert!(s.lookup(&key).is_some());
    // A definitively broken entry (checkout gone) is replaced.
    std::fs::remove_dir_all(h.src.join(".git")).unwrap();
    let h2 = materialize(&s, &key, meta);
    assert!(h2.src.join(".git").exists());
}

/// `keys-v1` markers: `create_new`, exists, retained by the caller's decision.
#[test]
fn markers_and_retention() {
    let (_g, _root, s) = new_store();
    let a = MarkerKey::Initialized {
        db_repo_name: Arc::from("https://x/y:a,b"),
        rev: Arc::from("v1"),
    };
    let b = MarkerKey::Installed {
        install_key: [9; 16],
    };
    assert!(!s.marker_exists(&a));
    s.set_marker(&a);
    s.set_marker(&a);
    s.set_marker(&b);
    assert!(s.marker_exists(&a) && s.marker_exists(&b));
    let mut seen = Vec::new();
    let removed = s.retain_markers(&mut |k| {
        seen.push(format!("{k:?}"));
        matches!(k, MarkerKey::Installed { .. })
    });
    assert_eq!(removed, 1);
    assert_eq!(seen.len(), 2);
    assert!(!s.marker_exists(&a) && s.marker_exists(&b));
    // A missing store never gets created by set_marker.
    let d = tempfile::tempdir().unwrap();
    let missing = store_at(&d.path().join("missing"));
    missing.set_marker(&a);
    assert!(!missing.exists());
}

/// `RepoId::from_config` resolves `repo: ../tools` against the given project root (not the process cwd), and the
/// two projects' keys differ (D-DATA-08).
#[test]
fn repo_id_from_config_uses_the_project_root() {
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    for p in ["a/proj", "a/tools", "b/proj", "b/tools"] {
        std::fs::create_dir_all(base.join(p)).unwrap();
    }
    let ida = RepoId::from_config("../tools", &base.join("a/proj"));
    let idb = RepoId::from_config("../tools", &base.join("b/proj"));
    assert!(matches!(&ida, RepoId::LocalPath(p) if *p == base.join("a/tools")));
    assert!(matches!(&idb, RepoId::LocalPath(p) if *p == base.join("b/tools")));
    assert_ne!(
        Store::remote_key(&ida, "HEAD", &[]),
        Store::remote_key(&idb, "HEAD", &[])
    );
    assert_eq!(
        snp_store::db_repo_name("../tools", &[Arc::from("b"), Arc::from("a")]),
        "../tools:b,a"
    );
}

/// Relocation: a store moved to another path reports the old one once `ensure_created` recorded the new path.
#[test]
fn relocated_store_is_detected() {
    let (g, root, s) = new_store();
    drop(s);
    let moved = g.path().join("moved");
    std::fs::rename(&root, &moved).unwrap();
    let moved = snp_os::realpath::realpath_nonstrict(&moved, Path::new("/")).unwrap();
    let s = store_at(&moved);
    assert_eq!(s.relocated_from().as_deref(), Some(root.as_path()));
    s.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(s.relocated_from().as_deref(), Some(root.as_path()));
    let again = store_at(&moved);
    again
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(again.relocated_from(), None);
}

/// Relocation invalidates the environments durably: the first process after the move removes every install state
/// before it records the new path, so a second process (which no longer sees the move) never trusts an environment
/// with the old absolute paths baked in. The clone itself is kept.
#[test]
fn relocation_invalidates_environments_for_every_later_process() {
    let (g, root, s) = new_store();
    let h = handle(&s);
    let env_dir = s.env_dir(&h, LanguageId::Python, "3.12");
    std::fs::create_dir_all(&env_dir).unwrap();
    Store::commit_install_marker(&env_dir, b"{\"v\":1}", &[]).unwrap();
    drop(s);
    let moved = g.path().join("moved");
    std::fs::rename(&root, &moved).unwrap();
    let moved = snp_os::realpath::realpath_nonstrict(&moved, Path::new("/")).unwrap();
    let moved_env = moved
        .join("repos-v1")
        .join(h.key.hex())
        .join("envs")
        .join(env_dir.file_name().unwrap());
    assert!(Store::read_install_marker(&moved_env).is_some());

    // Process 1 (e.g. `snp gc`) creates the store and consumes the notice.
    let first = store_at(&moved);
    first
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(first.relocated_from().as_deref(), Some(root.as_path()));
    // Process 2 sees no move, and no valid environment.
    let second = store_at(&moved);
    second
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(second.relocated_from(), None);
    assert_eq!(Store::read_install_marker(&moved_env), None);
    let (key, _) = remote(URL, "env", &[]);
    assert!(second.lookup(&key).is_some(), "the clone is kept");
}

/// Store paths: crash log by brand, spill/patches/scratch dirs, lazy `new`.
#[test]
fn paths_and_laziness() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("lazy");
    let s = store_at(&root);
    assert!(!s.exists());
    assert_eq!(s.crash_log(Brand::PreCommit), root.join("pre-commit.log"));
    assert_eq!(s.crash_log(Brand::Snp), root.join("snp.log"));
    assert_eq!(s.spill_dir(), root.join("spill"));
    assert!(!root.exists(), "no I/O before ensure_created");
    s.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("README")).unwrap(),
        "This directory is maintained by snp (You Shell Not Pass).\nLearn more: https://github.com/grok-rs/snp\n"
    );
    assert!(s.spill_dir().is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [
            "",
            "repos-v1",
            "spill",
            "patches",
            "scratch",
            "keys-v1",
            "configs-v1",
        ] {
            let mode = std::fs::metadata(root.join(dir))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{dir}");
        }
    }
    assert_eq!(s.location().path, root);
}

// ----- ownership policy -----

/// A world-writable store without the sticky bit is refused (ADR-044).
#[cfg(unix)]
#[test]
fn world_writable_store_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("ww");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    let err = store_at(&root)
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert!(
        exc_text(&err)
            .1
            .starts_with("snp refuses to use the world-writable store")
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o1777)).unwrap();
    store_at(&root)
        .ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
}

/// D-SEC-13 (posix, CI container): as root, a store owned by uid 1000 is refused unless
/// `SNP_ALLOW_FOREIGN_STORE=1`; then it is used with one warning (D-DIAG-05). Skipped when not root.
#[cfg(unix)]
#[test]
fn foreign_store_as_root_is_refused_unless_allowed() {
    if !is_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("foreign");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::chown(&root, Some(1000), Some(1000)).unwrap();
    let allowed = Env::capture_process()
        .get("SNP_ALLOW_FOREIGN_STORE")
        .is_some_and(|v| v == "1");
    let diag = CollectWarnings::new();
    let r = store_at(&root).ensure_created(Brand::Snp, &diag);
    if allowed {
        r.unwrap();
        let w = diag.take();
        assert!(
            w.iter().any(|l| l.contains("is owned by uid 1000")),
            "{w:?}"
        );
        return;
    }
    let err = r.unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(
        exc_text(&err),
        (
            "FatalError".to_owned(),
            format!(
                "snp refuses to run as root from a store owned by uid 1000: {}; set SNP_ALLOW_FOREIGN_STORE=1 to trust it",
                root.display()
            )
        )
    );
    // The same in a child with the escape hatch set.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "behaviour::foreign_store_as_root_is_refused_unless_allowed",
            "--test-threads=1",
        ])
        .env("SNP_ALLOW_FOREIGN_STORE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ----- config tracking on a fresh store, gc fast paths (C1, P20) -----

/// C1: pre-commit creates its store before `mark_config_used`, so a fresh-store `install-hooks --config c` tracks
/// `c` and a later gc keeps its repositories. snp's lazy store keeps the pair until `ensure_created` and records it
/// then; a store that is never created stays absent, and a config that does not exist is never recorded.
#[test]
fn configs_marked_before_the_store_exists_are_tracked_on_creation() {
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let cfg = proj.join("c.yaml");
    std::fs::write(&cfg, b"repos: []\n").unwrap();
    let want = vec![snp_store::TrackedConfig {
        path: cfg.clone(),
        project_root: proj.clone(),
    }];

    let s = store_at(&base.join("store"));
    s.mark_config_used(&cfg, &proj);
    s.mark_config_used(&cfg, &proj);
    s.mark_config_used(&proj.join("missing.yaml"), &proj);
    assert!(!s.exists(), "tracking never creates the store");
    assert!(s.tracked_configs().is_empty());
    s.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_eq!(s.tracked_configs(), want);
    let entries = std::fs::read_dir(base.join("store/configs-v1"))
        .unwrap()
        .count();
    assert_eq!(entries, 1, "recorded once");

    // A config deleted before the store is created is not recorded (re-checked at the flush).
    let gone = proj.join("gone.yaml");
    std::fs::write(&gone, b"repos: []\n").unwrap();
    let late = store_at(&base.join("late"));
    late.mark_config_used(&gone, &proj);
    std::fs::remove_file(&gone).unwrap();
    late.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert!(late.tracked_configs().is_empty());

    // A process that never creates the store leaves nothing behind.
    let never = store_at(&base.join("never"));
    never.mark_config_used(&cfg, &proj);
    drop(never);
    assert!(!base.join("never").exists());
}

/// C1 for try-repo: the user's config is marked on the main store, which try-repo's ephemeral store creates through
/// its own copy of the main store's handle; the pending config is recorded in the main store only.
#[test]
fn ephemeral_store_records_the_main_stores_pending_configs() {
    let d = tempfile::tempdir().unwrap();
    let base = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
    let cfg = base.join(".pre-commit-config.yaml");
    std::fs::write(&cfg, b"repos: []\n").unwrap();
    let main = store_at(&base.join("main"));
    main.mark_config_used(&cfg, &base);
    let e = Store::ephemeral(&base.join("eph"), Some(&main));
    e.ensure_created(Brand::Snp, &CollectWarnings::new())
        .unwrap();
    let tracked = store_at(&base.join("main")).tracked_configs();
    assert_eq!(tracked.len(), 1);
    assert_eq!(tracked.first().unwrap().path, cfg);
    assert!(
        e.tracked_configs().is_empty(),
        "never tracked in the throwaway store"
    );
}
