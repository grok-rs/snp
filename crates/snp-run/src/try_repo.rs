//! `try-repo` (`commands/try_repo.py`; 03 §12, ADR-059).
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use snp_cli::TryRepoArgs;
use snp_core::failure::Failure;
use snp_core::output::{CollectWarnings, Level, WarnSink as _};
use snp_git::{CloneMode, OwnedRepo};
use snp_store::{MarkerKey, RepoId, RepoKind, RepoMeta, Store};

use crate::pipeline::{self, Invocation};
use crate::{RunCtx, common, messages};

/// pre-commit's throwaway `Store(tempdir)` (03 §12): runs against `Store::ephemeral(tmp, Some(&cx.store))`, so the
/// `Initializing environment` line and the `Installing environment` triplet print on every try-repo; the temp
/// config is never tracked (the user's `-c` is tracked in the main store as for every git command); ADR-059.
pub fn try_repo(a: TryRepoArgs, cx: &RunCtx) -> Result<i32, Failure> {
    let (git, info) = common::discover(cx)?;
    let root = info.root;
    let user_config = common::adjust_config(&a.run.config, &cx.invocation_cwd, &root);
    common::mark_config_used(&cx.store, &root.join(&user_config), &root);
    let repo = adjust_repo(&a.repo, &cx.invocation_cwd, &root);
    let tmp = common::TempDir::new(&cx.env)?;
    // `_repo_ref`
    let (repo, rev) = if let Some(r) = &a.rev {
        (repo, r.to_string_lossy().into_owned())
    } else {
        let owned = OwnedRepo::open(
            Arc::clone(&cx.git_exe),
            &cx.env,
            Path::new(&repo),
            Arc::clone(&cx.git_backend),
        );
        let (path, rev) = owned.shadow_try_repo(&git, tmp.path())?;
        if path.as_os_str() != repo.as_os_str() {
            cx.printer.log(Level::Warning, messages::TRY_REPO_SHADOW);
        }
        (path.into_os_string(), rev)
    };
    let repo_s = repo.to_string_lossy().into_owned();
    let store = Store::ephemeral(tmp.path(), Some(&cx.store));
    let selectors: Vec<String> = a
        .run
        .hooks
        .iter()
        .filter(|h| !h.is_empty())
        .map(|h| h.to_string_lossy().into_owned())
        .collect();
    let ids = if selectors.is_empty() {
        manifest_ids(cx, &store, &root, &repo_s, &rev)?
    } else {
        selectors
    };
    let config_s = config_yaml(&repo_s, &rev, &ids)?;
    let config_path = tmp.path().join(".pre-commit-config.yaml");
    std::fs::write(&config_path, config_s.as_bytes())
        .map_err(|e| Failure::from_io(&e, Some(config_path.as_os_str()), None))?;
    let rule = messages::rule();
    cx.printer.write_line(rule.as_bytes());
    cx.printer.write_line(messages::USING_CONFIG.as_bytes());
    cx.printer.write_line(rule.as_bytes());
    cx.printer.write(config_s.as_bytes());
    cx.printer.write_line(rule.as_bytes());
    let mut run = a.run;
    run.config = config_path.into_os_string();
    let inv = Invocation {
        store: &store,
        track_config: false,
        direct: false,
        fixed_paths: true,
        hook_stdin: None,
    };
    let rc = pipeline::run_pipeline(&run, cx, &inv);
    drop(store);
    drop(tmp);
    rc
}

/// `args.repo` after `_adjust_args_and_chdir`: an existing path is made root-relative.
fn adjust_repo(repo: &OsStr, invocation_cwd: &Path, root: &Path) -> OsString {
    if invocation_cwd.join(repo).exists() {
        common::rebase_path(repo, invocation_cwd, root)
    } else if root.join(repo).exists() {
        common::rebase_path(repo, root, root)
    } else {
        repo.to_os_string()
    }
}

/// `store.clone(repo, ref)` + the manifest's ids sorted (duplicates kept), printing `[INFO] Initializing
/// environment for {repo}.` as pre-commit's store does before cloning.
fn manifest_ids(
    cx: &RunCtx,
    store: &Store,
    root: &Path,
    repo: &str,
    rev: &str,
) -> Result<Vec<String>, Failure> {
    store.ensure_created(cx.brand, &*cx.printer)?;
    let id = RepoId::from_config(repo, root);
    let key = Store::remote_key(&id, rev, &[]);
    let marker = MarkerKey::Initialized {
        db_repo_name: Arc::from(snp_store::db_repo_name(repo, &[])),
        rev: Arc::from(rev),
    };
    if !store.marker_exists(&marker) {
        cx.printer.log(Level::Info, &messages::initializing(repo));
    }
    let clone_src = match &id {
        RepoId::LocalPath(p) => p.to_string_lossy().into_owned(),
        _ => repo.to_owned(),
    };
    let meta = RepoMeta {
        kind: RepoKind::Remote {
            id,
            rev: Arc::from(rev),
        },
        deps: Arc::from(Vec::new()),
    };
    let mut fill = |dest: &Path| -> Result<(), Failure> {
        OwnedRepo::clone_rev(
            Arc::clone(&cx.git_exe),
            &cx.env,
            dest,
            &clone_src,
            rev,
            CloneMode::Foreground,
            Arc::clone(&cx.git_backend),
        )
        .map(drop)
    };
    let printer = Arc::clone(&cx.printer);
    let brand = cx.brand;
    let mut on_wait = move || printer.log(Level::Info, snp_store::locking_message(brand));
    let handle = store.materialize(&key, meta, &mut fill, &mut on_wait, &cx.cancel)?;
    store.set_marker(&marker);
    let warn = CollectWarnings::new();
    let opts = snp_config::LoadOpts {
        brand: cx.brand,
        ext: common::ext_mode(&cx.env),
        regexes: &cx.regexes,
        warn: &warn,
        diag: &warn,
    };
    let manifest = handle.src.join(".pre-commit-hooks.yaml");
    let m =
        snp_config::load_manifest(&manifest, manifest.as_os_str(), &opts).map_err(Failure::from)?;
    for w in warn.take() {
        cx.printer.warn(&w);
    }
    let mut ids: Vec<String> = m.hooks.iter().map(|h| h.id.to_string()).collect();
    ids.sort();
    Ok(ids)
}

/// `yaml_dump({'repos': [{'repo': repo, 'rev': ref, 'hooks': [{'id': …}, …]}]})`, byte-exact through snp-yaml's
/// `SafeDumper` (the value is built from a flow document whose scalars are all double-quoted).
fn config_yaml(repo: &str, rev: &str, ids: &[String]) -> Result<String, Failure> {
    let mut doc = String::from("{\"repos\": [{\"repo\": ");
    quote(&mut doc, repo);
    doc.push_str(", \"rev\": ");
    quote(&mut doc, rev);
    doc.push_str(", \"hooks\": [");
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            doc.push_str(", ");
        }
        doc.push_str("{\"id\": ");
        quote(&mut doc, id);
        doc.push('}');
    }
    doc.push_str("]}]}");
    let value = snp_yaml::load(&doc, &snp_yaml::Limits::default())
        .map_err(|e| Failure::unexpected("yaml.YAMLError", e.to_string()))?;
    Ok(snp_yaml::emit::dump(&value))
}

/// A YAML double-quoted scalar.
fn quote(out: &mut String, s: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 || c == '\u{7f}' => {
                write!(out, "\\x{:02x}", u32::from(c)).unwrap_or_default();
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t10_try_01_config_dump() {
        let y = config_yaml(
            "../hooks1",
            "aa658a0cb064bf927c7cbd4c3034e4d876de3c9c",
            &["h1".into(), "h2".into()],
        )
        .unwrap();
        assert_eq!(
            y,
            "repos:\n-   repo: ../hooks1\n    rev: aa658a0cb064bf927c7cbd4c3034e4d876de3c9c\n    hooks:\n    \
             -   id: h1\n    -   id: h2\n"
        );
        // scalars that need quoting stay exactly as PyYAML quotes them
        let y = config_yaml("a: b", "1.0", &["yes".into()]).unwrap();
        assert!(y.contains("repo: 'a: b'"), "{y}");
        assert!(y.contains("rev: '1.0'"), "{y}");
        assert!(y.contains("id: 'yes'"), "{y}");
        let y = config_yaml("x\"\\\t", "r", &[]).unwrap();
        assert!(y.contains("hooks: []"), "{y}");
    }

    #[test]
    fn t10_try_07_repo_rewritten_relative_to_root() {
        let base = std::env::temp_dir().join(format!("snp-try-{}", std::process::id()));
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::create_dir_all(base.join("foo")).unwrap();
        assert_eq!(
            adjust_repo(OsStr::new("../foo"), &base.join("sub"), &base),
            "foo"
        );
        assert_eq!(
            adjust_repo(OsStr::new("https://x/y"), &base.join("sub"), &base),
            "https://x/y"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
