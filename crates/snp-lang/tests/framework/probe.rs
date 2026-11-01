//! `ToolProbe` (WBS WP-16 acceptance: "a pyenv-style `#!` shim and a rustup proxy are never persisted; changing
//! `PYENV_VERSION`/`RUSTUP_TOOLCHAIN` invalidates a persisted native fact; per-process memoisation runs the probe once";
//! ADR-079).

use std::ffi::OsStr;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::{LangError, ToolProbe};

/// An executable with `content` at `p`.
fn exe(p: &Path, content: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A native-looking binary (ELF magic, never `#!`).
const NATIVE: &[u8] = b"\x7fELF\x02\x01\x01\0fake interpreter";

fn env_with_path(dir: &Path, extra: &[(&str, &str)]) -> Env {
    let mut pairs: Vec<(String, String)> =
        vec![("PATH".into(), dir.to_string_lossy().into_owned())];
    pairs.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    Env::from_pairs(pairs)
}

/// Calls `default_version` with a counting probe returning `value`.
fn probe_with(
    p: &ToolProbe,
    lang: LanguageId,
    env: &Env,
    cwd: &Path,
    runs: &AtomicUsize,
    value: &str,
) -> Arc<str> {
    p.default_version(lang, env, cwd, &mut || {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::from(value))
    })
    .unwrap()
}

/// The fact files written so far.
#[cfg(unix)]
fn facts(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default()
}

#[test]
fn memoised_per_process_the_probe_runs_once() {
    let d = tempfile::tempdir().unwrap();
    let p = ToolProbe::new(None);
    let env = env_with_path(d.path(), &[]);
    let runs = AtomicUsize::new(0);
    for _ in 0..3 {
        assert_eq!(
            &*probe_with(&p, LanguageId::Python, &env, d.path(), &runs, "python3.12"),
            "python3.12"
        );
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // Another language, another slot.
    probe_with(&p, LanguageId::Rust, &env, d.path(), &runs, "system");
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    // A different steering environment is a different fact.
    let other = env_with_path(d.path(), &[("PYENV_VERSION", "3.11")]);
    assert_eq!(
        &*probe_with(
            &p,
            LanguageId::Python,
            &other,
            d.path(),
            &runs,
            "python3.11"
        ),
        "python3.11"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 3);
    assert_eq!(
        &*probe_with(&p, LanguageId::Python, &env, d.path(), &runs, "x"),
        "python3.12"
    );
}

#[test]
fn concurrent_callers_share_one_probe() {
    let d = tempfile::tempdir().unwrap();
    let p = ToolProbe::new(None);
    let env = env_with_path(d.path(), &[]);
    let runs = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::Builder::new()
                    .spawn_scoped(s, || {
                        p.default_version(LanguageId::Node, &env, d.path(), &mut || {
                            runs.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(50));
                            Ok(Arc::from("system"))
                        })
                        .unwrap()
                    })
                    .unwrap()
            })
            .collect();
        for h in handles {
            assert_eq!(&*h.join().unwrap(), "system");
        }
    });
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_failing_probe_is_not_memoised() {
    let d = tempfile::tempdir().unwrap();
    let p = ToolProbe::new(None);
    let env = env_with_path(d.path(), &[]);
    let e = p
        .default_version(LanguageId::Rust, &env, d.path(), &mut || {
            Err(LangError::Interrupted)
        })
        .unwrap_err();
    assert!(matches!(e, LangError::Interrupted));
    let runs = AtomicUsize::new(0);
    assert_eq!(
        &*probe_with(&p, LanguageId::Rust, &env, d.path(), &runs, "default"),
        "default"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[test]
fn a_native_fact_is_persisted_and_invalidated_by_the_steering_env() {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("bin");
    let store = d.path().join("probes");
    exe(&bin.join("python3"), NATIVE);
    let env = env_with_path(&bin, &[]);
    let runs = AtomicUsize::new(0);

    let first = ToolProbe::new(Some(store.clone()));
    assert_eq!(
        &*probe_with(
            &first,
            LanguageId::Python,
            &env,
            d.path(),
            &runs,
            "python3.12"
        ),
        "python3.12"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(facts(&store).len(), 1);

    // A new process (new ToolProbe) reads the fact instead of probing.
    let second = ToolProbe::new(Some(store.clone()));
    assert_eq!(
        &*probe_with(&second, LanguageId::Python, &env, d.path(), &runs, "wrong"),
        "python3.12"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    // PYENV_VERSION, RUSTUP_TOOLCHAIN, ASDF_*/MISE_* steer the answer: a different fact.
    for (k, v) in [
        ("PYENV_VERSION", "3.13"),
        ("RUSTUP_TOOLCHAIN", "nightly"),
        ("ASDF_PYTHON_VERSION", "3.10"),
        ("MISE_PYTHON_VERSION", "3.9"),
        ("VIRTUAL_ENV", "/v"),
    ] {
        let steered = env_with_path(&bin, &[(k, v)]);
        let p = ToolProbe::new(Some(store.clone()));
        let before = runs.load(Ordering::SeqCst);
        assert_eq!(
            &*probe_with(&p, LanguageId::Python, &steered, d.path(), &runs, v),
            v,
            "{k}"
        );
        assert_eq!(runs.load(Ordering::SeqCst), before + 1, "{k}");
    }
    let rust_env = env_with_path(&bin, &[("RUSTUP_TOOLCHAIN", "1.80")]);
    exe(&bin.join("cargo"), NATIVE);
    let p = ToolProbe::new(Some(store.clone()));
    probe_with(&p, LanguageId::Rust, &env, d.path(), &runs, "system");
    let p = ToolProbe::new(Some(store.clone()));
    let before = runs.load(Ordering::SeqCst);
    probe_with(&p, LanguageId::Rust, &rust_env, d.path(), &runs, "system");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        before + 1,
        "RUSTUP_TOOLCHAIN invalidates the cargo fact"
    );

    // Replacing the interpreter (new size/mtime/inode) invalidates the fact.
    std::fs::remove_file(bin.join("python3")).unwrap();
    exe(
        &bin.join("python3"),
        b"\x7fELF a different build of the interpreter",
    );
    let p = ToolProbe::new(Some(store.clone()));
    let before = runs.load(Ordering::SeqCst);
    assert_eq!(
        &*probe_with(&p, LanguageId::Python, &env, d.path(), &runs, "python3.13"),
        "python3.13"
    );
    assert_eq!(runs.load(Ordering::SeqCst), before + 1);

    // A corrupt fact file is ignored and rewritten.
    for f in facts(&store) {
        std::fs::write(f, b"{not json").unwrap();
    }
    let p = ToolProbe::new(Some(store));
    let before = runs.load(Ordering::SeqCst);
    probe_with(&p, LanguageId::Python, &env, d.path(), &runs, "python3.13");
    assert_eq!(runs.load(Ordering::SeqCst), before + 1);
}

#[cfg(unix)]
#[test]
fn a_pyenv_style_shim_is_never_persisted() {
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("probes");
    // A `#!` script (pyenv/asdf/rbenv shims are bash scripts), in and outside a `shims` directory.
    for dir in ["bin", ".pyenv/shims"] {
        let bin = d.path().join(dir);
        exe(
            &bin.join("python3"),
            b"#!/usr/bin/env bash\nexec pyenv exec python3 \"$@\"\n",
        );
        let env = env_with_path(&bin, &[]);
        let runs = AtomicUsize::new(0);
        for _ in 0..2 {
            let p = ToolProbe::new(Some(store.clone()));
            probe_with(&p, LanguageId::Python, &env, d.path(), &runs, "python3.12");
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "{dir}: probed by every process"
        );
        assert_eq!(facts(&store), Vec::<PathBuf>::new());
    }
    // Even a native binary in a `shims` directory (scoop, mise) is a proxy.
    let shims = d.path().join("mise/shims");
    exe(&shims.join("python3"), NATIVE);
    let p = ToolProbe::new(Some(store.clone()));
    probe_with(
        &p,
        LanguageId::Python,
        &env_with_path(&shims, &[]),
        d.path(),
        &AtomicUsize::new(0),
        "v",
    );
    assert_eq!(facts(&store), Vec::<PathBuf>::new());
}

#[cfg(unix)]
#[test]
fn a_rustup_proxy_is_never_persisted() {
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("probes");
    // ~/.cargo/bin: `cargo` is a hard link/copy of `rustup`.
    let cargo_bin = d.path().join("cargo/bin");
    exe(&cargo_bin.join("rustup"), NATIVE);
    std::fs::hard_link(cargo_bin.join("rustup"), cargo_bin.join("cargo")).unwrap();
    // A symlink to rustup elsewhere.
    let links = d.path().join("links");
    std::fs::create_dir_all(&links).unwrap();
    std::os::unix::fs::symlink(cargo_bin.join("rustup"), links.join("cargo")).unwrap();
    for bin in [cargo_bin, links] {
        let env = env_with_path(&bin, &[]);
        let runs = AtomicUsize::new(0);
        for _ in 0..2 {
            let p = ToolProbe::new(Some(store.clone()));
            probe_with(&p, LanguageId::Rust, &env, d.path(), &runs, "system");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2, "{}", bin.display());
        assert_eq!(facts(&store), Vec::<PathBuf>::new());
    }
}

#[test]
fn find_exe_is_memoised_per_path() {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("bin");
    let name = if cfg!(windows) { "tool.exe" } else { "tool" };
    exe(&bin.join(name), NATIVE);
    let env = Env::from_pairs([
        ("PATH", bin.to_string_lossy().into_owned()),
        ("PATHEXT", ".EXE".to_owned()),
    ]);
    let p = ToolProbe::new(None);
    let found = p.find_exe(OsStr::new("tool"), &env, d.path()).unwrap();
    // pre-commit joins the name with the `PATHEXT` entry as spelt (`tool.EXE`); NTFS matches it case-insensitively.
    let spelt = if cfg!(windows) { "tool.EXE" } else { "tool" };
    assert_eq!(found, bin.join(spelt));
    std::fs::remove_file(bin.join(name)).unwrap();
    assert_eq!(
        p.find_exe(OsStr::new("tool"), &env, d.path()),
        Some(found),
        "memoised"
    );
    let other = Env::from_pairs([("PATH", d.path().to_string_lossy().into_owned())]);
    assert_eq!(p.find_exe(OsStr::new("tool"), &other, d.path()), None);
}

#[test]
fn fingerprints() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("python");
    exe(&f, NATIVE);
    let p = ToolProbe::new(None);
    let fp = p.fingerprint(&f).unwrap();
    assert_eq!(fp.exe, f);
    assert_eq!(fp.len, u64::try_from(NATIVE.len()).unwrap());
    assert!(fp.mtime_ns > 0);
    assert_eq!(&*fp.detail, "");
    assert_eq!(p.fingerprint(&f).as_ref(), Some(&fp), "stable");
    assert_eq!(p.fingerprint(&d.path().join("missing")), None);
    #[cfg(unix)]
    {
        // Symlinks are followed (a venv's python points at the base interpreter).
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        let lf = p.fingerprint(&link).unwrap();
        assert_eq!(
            (lf.len, lf.mtime_ns, lf.ino_or_index),
            (fp.len, fp.mtime_ns, fp.ino_or_index)
        );
    }
}
