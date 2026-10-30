//! Catalog rows owned by WP-22 in snp-store (ARCHITECTURE §13.8, `tests/catalog/index.tsv`): T10-STO-01, 02, 06,
//! 09, 10, 11, 12 (pre-commit's `tests/store_test.py`, adapted to snp's own store) and X13-ST-01…12 except ST-04
//! (13 X.4.2: the SQLite rows become "snp has no database"). Each test is named after its row. The rest of the
//! WP-22 acceptance list lives in `support/behaviour.rs` (one test binary, shared helpers).
//!
//! Owner: WP-22.

#[cfg(test)]
#[path = "support/common.rs"]
mod common;

#[cfg(test)]
#[path = "support/behaviour.rs"]
mod behaviour;

#[cfg(test)]
mod catalog {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use snp_core::brand::Brand;
    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::output::CollectWarnings;
    use snp_store::{Store, StoreSource, TrackedConfig, resolve_location};

    use crate::common::{exc_text, fake_clone, is_root, materialize, new_store, remote, store_at};

    const URL: &str = "https://example.invalid/catalog";

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    fn real(p: &Path) -> PathBuf {
        snp_os::realpath::realpath_nonstrict(p, Path::new("/")).unwrap()
    }

    /// T10-STO-01: default `realpath(~/.cache/snp)`; `XDG_CACHE_HOME=/tmp/fakehome` → `/tmp/fakehome/snp`;
    /// `SNP_HOME` → its realpath (precedence `SNP_HOME` > `XDG_CACHE_HOME` > `~/.cache`).
    #[cfg(unix)]
    #[test]
    fn t10_sto_01_default_directory_precedence() {
        let d = tempfile::tempdir().unwrap();
        let home = real(d.path());
        let h = home.to_str().unwrap();
        let cwd = Path::new("/");
        let loc = resolve_location(&env(&[("HOME", h)]), cwd).unwrap();
        assert_eq!(loc.path, home.join(".cache/snp"));
        assert_eq!(loc.source, StoreSource::PlatformDefault);
        let loc = resolve_location(&env(&[("HOME", h), ("XDG_CACHE_HOME", "/tmp/fakehome")]), cwd).unwrap();
        assert_eq!(loc.path, real(Path::new("/tmp/fakehome")).join("snp"));
        let snp_home = home.join("snp_home");
        let loc = resolve_location(
            &env(&[
                ("HOME", h),
                ("XDG_CACHE_HOME", "/tmp/fakehome"),
                ("SNP_HOME", snp_home.to_str().unwrap()),
            ]),
            cwd,
        )
        .unwrap();
        assert_eq!(loc.path, snp_home);
        assert_eq!(loc.source, StoreSource::SnpHome);
    }

    /// T10-STO-02: the store directory is created with snp's README.
    #[test]
    fn t10_sto_02_store_created_with_readme() {
        let (_g, root, _s) = new_store();
        let readme = std::fs::read_to_string(root.join("README")).unwrap();
        for line in [
            "This directory is maintained by snp (You Shell Not Pass).",
            "Learn more: https://github.com/grok-rs/snp",
        ] {
            assert!(readme.contains(line), "{readme}");
        }
    }

    /// T10-STO-06: an existing entry is returned without cloning.
    #[test]
    fn t10_sto_06_existing_entry_is_returned_without_cloning() {
        let (_g, _root, s) = new_store();
        let (key, meta) = remote(URL, "fake_ref", &[]);
        let first = materialize(&s, &key, meta.clone());
        let mut no_clone = |_: &Path| -> Result<(), Failure> { panic!("cloned again") };
        let again = s
            .materialize(&key, meta, &mut no_clone, &mut || {}, &CancelToken::new())
            .unwrap();
        assert_eq!(again.src, first.src);
        assert_eq!(s.lookup(&key).unwrap().src, first.src);
    }

    /// T10-STO-09: a directory without the store files gets them; re-opening is idempotent.
    #[test]
    fn t10_sto_09_existing_directory_without_store_files() {
        let d = tempfile::tempdir().unwrap();
        let root = real(d.path()).join("pre-existing");
        std::fs::create_dir_all(&root).unwrap();
        let s = store_at(&root);
        s.ensure_created(Brand::Snp, &CollectWarnings::new()).unwrap();
        assert!(root.join("store-v1.json").is_file());
        assert!(root.join("repos-v1").is_dir());
        s.ensure_created(Brand::Snp, &CollectWarnings::new()).unwrap();
        let again = store_at(&root);
        again.ensure_created(Brand::PreCommit, &CollectWarnings::new()).unwrap();
        assert_eq!(again.relocated_from(), None);
    }

    /// T10-STO-10: `db_repo_name('repo', ())` = `repo`; `('b','a','c')` → `repo:b,a,c` (order preserved).
    #[test]
    fn t10_sto_10_db_repo_name_keeps_order() {
        assert_eq!(snp_store::db_repo_name("repo", &[]), "repo");
        let deps: Vec<Arc<str>> = ["b", "a", "c"].iter().map(|d| Arc::from(*d)).collect();
        assert_eq!(snp_store::db_repo_name("repo", &deps), "repo:b,a,c");
        // Order is part of the key too.
        let id = snp_store::RepoId::Url(Arc::from("repo"));
        let mut sorted = deps.clone();
        sorted.sort();
        assert_ne!(Store::remote_key(&id, "r", &deps), Store::remote_key(&id, "r", &sorted));
    }

    /// T10-STO-11: `mark_config_used` records the absolute path (with its project root); idempotent; a nonexistent
    /// path is ignored; a missing `configs-v1` is recreated.
    #[test]
    fn t10_sto_11_mark_config_used() {
        let (g, root, s) = new_store();
        let proj = real(g.path()).join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let cfg = proj.join(".pre-commit-config.yaml");
        std::fs::write(&cfg, b"repos: []\n").unwrap();
        s.mark_config_used(&cfg, &proj);
        let want = vec![TrackedConfig {
            path: cfg.clone(),
            project_root: proj.clone(),
        }];
        assert_eq!(s.tracked_configs(), want);
        s.mark_config_used(&cfg, &proj);
        assert_eq!(s.tracked_configs(), want);
        s.mark_config_used(&proj.join("missing.yaml"), &proj);
        assert_eq!(s.tracked_configs(), want);
        std::fs::remove_dir_all(root.join("configs-v1")).unwrap();
        s.mark_config_used(&cfg, &proj);
        assert_eq!(s.tracked_configs(), want);
        s.forget_configs(&[cfg]);
        assert_eq!(s.tracked_configs().len(), 0);
    }

    /// T10-STO-12: a read-only store (posix) is used as is; `mark_config_used` is a no-op.
    #[cfg(unix)]
    #[test]
    fn t10_sto_12_read_only_store_skips_config_tracking() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let (g, root, s) = new_store();
        let cfg = g.path().join("f");
        std::fs::write(&cfg, b"").unwrap();
        let ro = |mode| {
            for dir in ["", "configs-v1"] {
                std::fs::set_permissions(root.join(dir), std::fs::Permissions::from_mode(mode)).unwrap();
            }
        };
        ro(0o500);
        let again = store_at(&root);
        again.ensure_created(Brand::Snp, &CollectWarnings::new()).unwrap();
        again.mark_config_used(&cfg, g.path());
        assert_eq!(again.tracked_configs().len(), 0);
        ro(0o700);
        drop(s);
    }

    /// X13-ST-01: a store that cannot be created affects only commands that need it: `Store::new`, paths and the
    /// crash-log path need no I/O; `ensure_created` fails with the `mkdir` `PermissionError` (exit 3), once.
    #[cfg(unix)]
    #[test]
    fn x13_st_01_store_cannot_be_created() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let parent = real(d.path()).join("ro");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let root = parent.join("pch");
        let s = store_at(&root);
        assert!(!s.exists());
        assert_eq!(s.crash_log(Brand::PreCommit), root.join("pre-commit.log"));
        let _patches = s.patches_dir();
        let err = s.ensure_created(Brand::PreCommit, &CollectWarnings::new()).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert_eq!(
            exc_text(&err),
            (
                "PermissionError".to_owned(),
                format!("[Errno 13] Permission denied: '{}'", root.display())
            )
        );
        // Nothing else is attempted, nothing panics: config tracking just skips.
        s.mark_config_used(&root, &root);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// X13-ST-02: the store path names a regular file → `NotADirectoryError: … '<file>/.lock'` (exit 3).
    #[cfg(unix)]
    #[test]
    fn x13_st_02_store_path_is_a_regular_file() {
        let d = tempfile::tempdir().unwrap();
        let file = real(d.path()).join("file");
        std::fs::write(&file, b"").unwrap();
        let err = store_at(&file)
            .ensure_created(Brand::PreCommit, &CollectWarnings::new())
            .unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert_eq!(
            exc_text(&err),
            (
                "NotADirectoryError".to_owned(),
                format!("[Errno 20] Not a directory: '{}/.lock'", file.display())
            )
        );
    }

    /// X13-ST-03: a read-only store with a new clone needed fails with `PermissionError` (exit 3); cached entries
    /// still work.
    #[cfg(unix)]
    #[test]
    fn x13_st_03_read_only_store_needs_a_new_clone() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let (_g, root, s) = new_store();
        let (cached, meta) = remote(URL, "cached", &[]);
        materialize(&s, &cached, meta);
        let set = |mode| {
            for dir in ["", "scratch", "repos-v1"] {
                std::fs::set_permissions(root.join(dir), std::fs::Permissions::from_mode(mode)).unwrap();
            }
        };
        set(0o500);
        let ro = store_at(&root);
        ro.ensure_created(Brand::Snp, &CollectWarnings::new()).unwrap();
        assert!(ro.lookup(&cached).is_some());
        let (key, meta) = remote(URL, "new", &["six"]);
        let err = ro
            .materialize(&key, meta, &mut fake_clone, &mut || {}, &CancelToken::new())
            .unwrap_err();
        set(0o700);
        assert_eq!(err.exit_code(), 3);
        assert_eq!(exc_text(&err).0, "PermissionError");
    }

    /// X13-ST-05: no database lock can block snp: config tracking is lock-free even while gc holds the exclusive
    /// lock.
    #[test]
    fn x13_st_05_config_tracking_never_waits_for_a_lock() {
        let (g, _root, s) = new_store();
        let _gc = s.exclusive(&mut || {}, &CancelToken::new()).unwrap();
        let cfg = g.path().join("c.yaml");
        std::fs::write(&cfg, b"").unwrap();
        let start = std::time::Instant::now();
        s.mark_config_used(&cfg, g.path());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(s.tracked_configs().len(), 1);
    }

    /// X13-ST-06/07/08: a `db.db` that is garbage, empty or a directory (pre-commit: `DatabaseError` /
    /// `OperationalError`, exit 3) is not snp's: never opened, never touched.
    fn db_db_is_ignored(make: impl FnOnce(&Path)) {
        let (g, root, s) = new_store();
        make(&root.join("db.db"));
        let (key, meta) = remote(URL, "db", &[]);
        materialize(&s, &key, meta);
        let cfg = g.path().join("cfg.yaml");
        std::fs::write(&cfg, b"").unwrap();
        s.mark_config_used(&cfg, g.path());
        assert_eq!(s.tracked_configs().len(), 1);
        assert!(root.join("db.db").exists());
        let _swept = s.sweep(std::time::SystemTime::now());
        assert!(root.join("db.db").exists(), "only 1.x leftovers behind their README are removed");
    }

    #[test]
    fn x13_st_06_db_db_not_a_database_is_ignored() {
        db_db_is_ignored(|p| std::fs::write(p, b"garbage, not sqlite").unwrap());
    }

    #[test]
    fn x13_st_07_db_db_empty_is_ignored() {
        db_db_is_ignored(|p| std::fs::write(p, b"").unwrap());
    }

    #[test]
    fn x13_st_08_db_db_directory_is_ignored() {
        db_db_is_ignored(|p| std::fs::create_dir_all(p).unwrap());
    }

    /// X13-ST-09: tracking storage that is read-only inside a writable store: best effort, silent.
    #[cfg(unix)]
    #[test]
    fn x13_st_09_read_only_tracking_inside_writable_store() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let (g, root, s) = new_store();
        let configs = root.join("configs-v1");
        std::fs::set_permissions(&configs, std::fs::Permissions::from_mode(0o500)).unwrap();
        let cfg = g.path().join("c.yaml");
        std::fs::write(&cfg, b"").unwrap();
        s.mark_config_used(&cfg, g.path());
        assert_eq!(s.tracked_configs().len(), 0);
        std::fs::set_permissions(&configs, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// X13-ST-10: a stale entry whose checkout was deleted is "not materialised" and is replaced (pre-commit:
    /// `InvalidManifestError` / `CalledProcessError`).
    #[test]
    fn x13_st_10_stale_entry_is_replaced() {
        let (_g, root, s) = new_store();
        let (key, meta) = remote(URL, "stale", &[]);
        let h = materialize(&s, &key, meta.clone());
        std::fs::remove_dir_all(&h.src).unwrap();
        assert!(s.lookup(&key).is_none());
        let h2 = materialize(&s, &key, meta.clone());
        assert!(h2.src.join(".git").is_dir());
        // A garbled marker is the same.
        std::fs::write(root.join("repos-v1").join(key.hex()).join(".snp-repo.json"), b"{").unwrap();
        assert!(s.lookup(&key).is_none());
        assert_eq!(s.all_repos().len(), 0);
        let h3 = materialize(&s, &key, meta);
        assert!(h3.src.join(".pre-commit-hooks.yaml").is_file());
        assert_eq!(s.all_repos().len(), 1);
    }

    /// X13-ST-11: `<store>/.lock` cannot be opened (a directory) → `IsADirectoryError` naming it (exit 3).
    #[cfg(unix)]
    #[test]
    fn x13_st_11_lock_file_is_a_directory() {
        let (_g, root, s) = new_store();
        std::fs::create_dir_all(root.join(".lock")).unwrap();
        let err = s.shared(&mut || {}, &CancelToken::new()).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert_eq!(
            exc_text(&err),
            (
                "IsADirectoryError".to_owned(),
                format!("[Errno 21] Is a directory: '{}'", root.join(".lock").display())
            )
        );
        assert!(s.exclusive(&mut || {}, &CancelToken::new()).is_err());
    }

    /// X13-ST-12: lock failures other than contention are `OSError`s (exit 3), never panics (snp locks with
    /// `File::lock`, `flock`/`LockFileEx`, not `msvcrt.locking`).
    #[cfg(unix)]
    #[test]
    fn x13_st_12_lock_errors_are_os_errors() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let (_g, root, s) = new_store();
        std::fs::write(root.join(".lock"), b"").unwrap();
        std::fs::set_permissions(root.join(".lock"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = s.exclusive(&mut || {}, &CancelToken::new()).unwrap_err();
        std::fs::set_permissions(root.join(".lock"), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(err.exit_code(), 3);
        assert_eq!(exc_text(&err).0, "PermissionError");
        assert!(s.exclusive(&mut || {}, &CancelToken::new()).is_ok());
    }
}
