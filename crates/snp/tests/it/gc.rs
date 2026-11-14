//! Integration tests: `gc` (T10-GC-01…07, ADR-075 marking).
//!
//! Store entries are materialised through `snp_store`'s own API (what WP-31's clones do), so gc's marking is
//! tested without the run pipeline.
//!
//! Owner: WP-33.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::cancel::CancelToken;
use snp_core::output::CollectWarnings;
use snp_store::{MarkerKey, RepoId, RepoKind, RepoMeta, Store, StoreLocation, StoreSource};
use snp_testkit::TestEnv;

use crate::cli::{LOCAL_CONFIG, assert_code, compat, git, repo_with, store, tenv, text};

const MANIFEST: &str = "-   id: foo\n    name: foo\n    entry: echo\n    language: system\n";

fn open_store(env: &TestEnv) -> Store {
    let s = Store::new(StoreLocation {
        path: store(env),
        source: StoreSource::SnpHome,
    });
    s.ensure_created(snp_core::brand::Brand::Snp, &CollectWarnings::new())
        .unwrap();
    s
}

/// Materialises `(repo, rev, deps)` with `manifest` (None: no manifest file) as a clone would.
fn materialize(
    st: &Store,
    repo: &str,
    root: &Path,
    rev: &str,
    deps: &[&str],
    manifest: Option<&str>,
) -> PathBuf {
    let id = RepoId::from_config(repo, root);
    let deps: Arc<[Arc<str>]> = deps.iter().map(|d| Arc::from(*d)).collect();
    let key = Store::remote_key(&id, rev, &deps);
    let meta = RepoMeta {
        kind: RepoKind::Remote {
            id,
            rev: Arc::from(rev),
        },
        deps,
    };
    let manifest = manifest.map(str::to_owned);
    let h = st
        .materialize(
            &key,
            meta,
            &mut |dir: &Path| {
                std::fs::create_dir_all(dir.join(".git")).unwrap();
                if let Some(m) = &manifest {
                    std::fs::write(dir.join(".pre-commit-hooks.yaml"), m).unwrap();
                }
                Ok(())
            },
            &mut || {},
            &CancelToken::new(),
        )
        .unwrap();
    h.src
}

fn config(repo: &str, rev: &str) -> String {
    format!("repos:\n-   repo: {repo}\n    rev: {rev}\n    hooks:\n    -   id: foo\n")
}

fn track(st: &Store, r: &Path) {
    let cfg = snp_os::realpath::realpath_nonstrict(&r.join(".pre-commit-config.yaml"), r).unwrap();
    st.mark_config_used(&cfg, r);
}

fn gc(env: &TestEnv, cwd: &Path) -> String {
    let out = compat(env, cwd, &["gc"]);
    assert_code(&out, 0);
    text(&out.stdout)
}

#[test]
fn t10_gc_01_old_rev_collected_then_config_removed() {
    let env = tenv();
    let st = open_store(&env);
    let up = "https://example.com/hooks";
    let r = repo_with(&env, "r", &config(up, "v2.0.0"));
    materialize(&st, up, &r, "v1.0.0", &[], Some(MANIFEST));
    materialize(&st, up, &r, "v2.0.0", &[], Some(MANIFEST));
    track(&st, &r);
    assert_eq!(st.all_repos().len(), 2);
    assert_eq!(gc(&env, env.root()), "1 repo(s) removed.\n");
    assert_eq!(st.all_repos().len(), 1);
    assert_eq!(st.tracked_configs().len(), 1);
    std::fs::remove_file(r.join(".pre-commit-config.yaml")).unwrap();
    assert_eq!(gc(&env, env.root()), "1 repo(s) removed.\n");
    assert!(st.all_repos().is_empty() && st.tracked_configs().is_empty());
}

#[test]
fn t10_gc_02_nothing_to_remove() {
    let env = tenv();
    let st = open_store(&env);
    for (name, cfg) in [
        ("never", config("https://example.com/never", "v1.0.0")),
        (
            "meta",
            "repos:\n-   repo: meta\n    hooks:\n    -   id: identity\n".to_owned(),
        ),
        ("local", LOCAL_CONFIG.to_owned()),
    ] {
        let r = repo_with(&env, name, &cfg);
        track(&st, &r);
    }
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
}

#[test]
fn t10_gc_03_local_repo_kept_while_config_exists() {
    let env = tenv();
    let st = open_store(&env);
    let cfg = "repos:\n-   repo: local\n    hooks:\n    -   id: l\n        name: l\n        entry: l\n        language: python\n        additional_dependencies: [six]\n";
    let r = repo_with(&env, "r", cfg);
    track(&st, &r);
    let deps: Arc<[Arc<str>]> = Arc::from(vec![Arc::<str>::from("six")]);
    let key = Store::local_key(&deps);
    st.materialize(
        &key,
        RepoMeta {
            kind: RepoKind::Local,
            deps,
        },
        &mut |d: &Path| {
            std::fs::create_dir_all(d).unwrap();
            Ok(())
        },
        &mut || {},
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
    assert!(st.lookup(&key).is_some());
    std::fs::remove_file(r.join(".pre-commit-config.yaml")).unwrap();
    assert_eq!(gc(&env, env.root()), "1 repo(s) removed.\n");
}

#[test]
fn t10_gc_04_unknown_hook_id_does_not_crash() {
    let env = tenv();
    let st = open_store(&env);
    let up = "https://example.com/hooks";
    let r = repo_with(
        &env,
        "r",
        &format!(
            "repos:\n-   repo: {up}\n    rev: v1.0.0\n    hooks:\n    -   id: does-not-exist\n"
        ),
    );
    materialize(&st, up, &r, "v1.0.0", &[], Some(MANIFEST));
    track(&st, &r);
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
    assert_eq!(st.all_repos().len(), 1);
}

#[test]
fn t10_gc_05_invalid_config_is_forgotten() {
    let env = tenv();
    let st = open_store(&env);
    let r = repo_with(&env, "r", "i am: invalid\n");
    track(&st, &r);
    assert_eq!(st.tracked_configs().len(), 1);
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
    assert_eq!(st.tracked_configs().len(), 0);
}

#[test]
fn t10_gc_06_manifest_deleted_repo_removed() {
    let env = tenv();
    let st = open_store(&env);
    let up = "https://example.com/hooks";
    let r = repo_with(&env, "r", &config(up, "v1.0.0"));
    let src = materialize(&st, up, &r, "v1.0.0", &[], Some(MANIFEST));
    std::fs::remove_file(src.join(".pre-commit-hooks.yaml")).unwrap();
    track(&st, &r);
    assert_eq!(gc(&env, env.root()), "1 repo(s) removed.\n");
}

#[test]
fn t10_gc_07_store_without_tracked_configs() {
    let env = tenv();
    let _st = open_store(&env);
    drop(std::fs::remove_dir_all(store(&env).join("configs-v1")));
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
    // and a store that does not exist yet is not created
    let env = tenv();
    let missing = env.root().join("no-store");
    let env = env.with_env("SNP_HOME", missing.as_os_str());
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
    assert!(!missing.exists());
}

/// ADR-075: a Tier A (pre-commit-hooks at a known tag) repository has no `repos-v1` entry but keeps its markers;
/// an unused marker is swept.
#[test]
fn tier_a_markers_are_kept() {
    let env = tenv();
    let st = open_store(&env);
    let up = "https://github.com/pre-commit/pre-commit-hooks";
    let r = repo_with(
        &env,
        "r",
        &format!("repos:\n-   repo: {up}\n    rev: v4.6.0\n    hooks:\n    -   id: check-yaml\n"),
    );
    track(&st, &r);
    let kept = MarkerKey::Initialized {
        db_repo_name: Arc::from(up),
        rev: Arc::from("v4.6.0"),
    };
    let stale = MarkerKey::Initialized {
        db_repo_name: Arc::from("https://example.com/gone"),
        rev: Arc::from("v1.0.0"),
    };
    st.set_marker(&kept);
    st.set_marker(&stale);
    assert_eq!(gc(&env, env.root()), "0 repo(s) removed.\n");
    assert!(st.marker_exists(&kept));
    assert!(!st.marker_exists(&stale));
}

/// ADR-075: `repo: ../tools` of a tracked config resolves against its recorded project root, whatever gc's cwd.
#[test]
fn relative_local_repo_resolves_against_the_project_root() {
    let env = tenv();
    let st = open_store(&env);
    let tools = env
        .git_repo("tools")
        .file(".pre-commit-hooks.yaml", MANIFEST.as_bytes())
        .stage_all()
        .commit("m")
        .build();
    let project = repo_with(&env, "proj", &config("../tools", "v1.0.0"));
    let (_, _, _) = git(&env, &tools, &["tag", "v1.0.0"]);
    materialize(&st, "../tools", &project, "v1.0.0", &[], Some(MANIFEST));
    track(&st, &project);
    let elsewhere = env.root().join("home");
    assert_eq!(gc(&env, &elsewhere), "0 repo(s) removed.\n");
    assert_eq!(st.all_repos().len(), 1);
}

/// D-DATA-04, D-DIAG-05: a stash journal that cannot be read keeps every stash patch, and gc and clean say so on
/// stderr (stdout and the exit code are unchanged).
#[test]
fn an_unreadable_stash_journal_keeps_every_patch_with_a_warning() {
    let env = tenv();
    let st = open_store(&env);
    let patches = st.patches_dir();
    let journal = patches.join("run-9-abcd.snp-journal");
    std::fs::create_dir_all(&journal).unwrap();
    std::fs::write(patches.join("patch1700000000-9"), b"maybe the only copy").unwrap();
    let f = std::fs::File::options()
        .write(true)
        .open(patches.join("patch1700000000-9"))
        .unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_hours(2))
        .unwrap();
    drop(f);
    let warning = format!(
        "[WARNING] the stash journal {} cannot be read or understood; every stash patch is kept. \
         Remove the journal once no snp run uses it and no patch it may name is needed.\n",
        journal.display()
    );
    let out = compat(&env, env.root(), &["gc"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "0 repo(s) removed.\n");
    assert_eq!(text(&out.stderr), warning);
    let out = compat(&env, env.root(), &["clean"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stderr), warning);
    assert!(patches.join("patch1700000000-9").exists());
}
