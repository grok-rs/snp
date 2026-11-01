//! `autoupdate` (`commands/autoupdate.py`; 03 §11).
//!
//! Owner: WP-33.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use snp_cli::AutoupdateArgs;
use snp_config::revline::{self, RevUpdate, Rewrite};
use snp_config::{RepoConfig, load_config, load_manifest};
use snp_core::failure::{Failure, PyExc};
use snp_git::{GitExe, OwnedRepo, RevInfo};
use snp_pyre::RegexCache;

use crate::config_cmds::{migrate_file, write_config};
use crate::frame::{self, Frame};
use crate::{CmdCtx, messages};

/// One updatable repository of the config (`autoupdate.py:174-177`: not `local`, not `meta`).
#[derive(Debug, Clone)]
struct ConfigRepo {
    /// Index into the config's `repos` list (the rev-line rewrite's position).
    index: usize,
    /// `repo` as written.
    repo: Arc<str>,
    /// `rev` as written.
    rev: Arc<str>,
    /// The config's hook ids, in order.
    hook_ids: Vec<Arc<str>>,
}

/// The outcome of `_update_one` for one repository.
#[derive(Debug)]
enum Updated {
    /// `RevInfo.update` succeeded and every config hook still exists.
    Rev(RevInfo),
    /// `RepositoryCannotBeUpdatedError` — and, in snp, any failure of the repository's git commands (D-CRASH-06):
    /// the line printed after `[{repo}] ` (exit 1, the other repositories are still written).
    CannotUpdate(Vec<u8>),
}

/// `jobs = jobs or cpu_count(); min(jobs, len(repos) or len(config_repos)); max(jobs, 1)` (`autoupdate.py:192-195`).
fn worker_count(jobs: i64, selected: usize, all: usize) -> usize {
    let wanted: i64 = if jobs == 0 {
        i64::try_from(snp_proc::xargs::cpu_count().get()).unwrap_or(i64::MAX)
    } else {
        jobs
    };
    let bound = i64::try_from(if selected == 0 { all } else { selected }).unwrap_or(i64::MAX);
    usize::try_from(wanted.min(bound).max(1)).unwrap_or(1)
}

/// The `repo` argument `git.init_repo` receives: `os.path.abspath(remote)` when it names a local directory
/// (relative to the repository root, pre-commit's cwd), else the string verbatim.
fn remote_arg(repo: &str, root: &Path) -> String {
    let p = root.join(repo);
    if p.is_dir() {
        snp_pycompat::ospath::abspath(OsStr::new(repo), root)
            .to_string_lossy()
            .into_owned()
    } else {
        repo.to_owned()
    }
}

/// The text a failure of one repository's update prints after `[{repo}] `: the exception's text (a
/// `CalledProcessError` block for a failed `git fetch`, X13-GIT-02).
fn failure_text(f: &Failure) -> Vec<u8> {
    match f {
        Failure::Exc { exc, .. } => exc.text.clone(),
        Failure::SystemExit { stderr, .. } => stderr.clone(),
        other => format!("{other:?}").into_bytes(),
    }
}

/// `_update_one` (`autoupdate.py:103-113`): `RevInfo.update` in a fresh temporary directory (the store is not
/// used), then the manifest and `_check_hooks_still_exist_at_rev`.
fn update_one(
    cx: &CmdCtx<'_>,
    exe: &Arc<GitExe>,
    root: &Path,
    r: &ConfigRepo,
    tags_only: bool,
    freeze: bool,
) -> Result<Updated, Failure> {
    let tmp = tempfile::Builder::new()
        .prefix("tmp")
        .tempdir()
        .map_err(|e| Failure::from_io(&e, None, None))?;
    let owned = OwnedRepo::open(Arc::clone(exe), cx.env, tmp.path(), Arc::clone(&cx.git_backend));
    let mut info = match owned.rev_info(&remote_arg(&r.repo, root), tags_only, freeze) {
        Ok(info) => info,
        Err(f @ (Failure::Interrupted(_) | Failure::StdoutClosed)) => return Err(f),
        Err(f) => return Ok(Updated::CannotUpdate(failure_text(&f))),
    };
    let manifest_path = tmp.path().join(".pre-commit-hooks.yaml");
    let regexes = RegexCache::default();
    let o = frame::load_opts(cx, &regexes, frame::ext_mode(cx));
    let manifest = match load_manifest(&manifest_path, manifest_path.as_os_str(), &o) {
        Ok(m) => m,
        Err(e) if e.uncaught().is_some() => return Err(e.into()),
        Err(e) => return Ok(Updated::CannotUpdate(e.rendered.into_bytes())),
    };
    info.hook_ids = manifest.hooks.iter().map(|h| h.id.to_string()).collect();
    let have: BTreeSet<&str> = info.hook_ids.iter().map(String::as_str).collect();
    let missing: BTreeSet<&str> = r
        .hook_ids
        .iter()
        .map(|h| &**h)
        .filter(|h| !have.contains(h))
        .collect();
    if !missing.is_empty() {
        let sorted: Vec<&str> = missing.into_iter().collect();
        return Ok(Updated::CannotUpdate(
            messages::missing_hooks(&sorted).into_bytes(),
        ));
    }
    Ok(Updated::Rev(info))
}

/// Runs `update_one` for `todo` on `workers` threads, handing each result to `on_result` in completion order
/// (`as_completed`; config order with one worker). The first error of `on_result` or of a worker stops new work.
fn run_parallel(
    cx: &CmdCtx<'_>,
    f: &Frame,
    todo: &[&ConfigRepo],
    workers: usize,
    a: &AutoupdateArgs,
    on_result: &mut dyn FnMut(&ConfigRepo, Updated) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let next = AtomicUsize::new(0);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let root = f.root();
    std::thread::scope(|s| -> Result<(), Failure> {
        let (tx, rx) = mpsc::channel::<(usize, Result<Updated, Failure>)>();
        let mut spawned = 0_usize;
        for _ in 0..workers {
            let tx = tx.clone();
            let (next, stop) = (&next, &stop);
            let worker = move || {
                loop {
                    if stop.load(Ordering::Acquire) || cx.cancel.is_cancelled() {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::AcqRel);
                    let Some(r) = todo.get(i) else {
                        return;
                    };
                    let res = update_one(cx, &f.exe, root, r, !a.bleeding_edge, a.freeze);
                    if tx.send((i, res)).is_err() {
                        return;
                    }
                }
            };
            match std::thread::Builder::new()
                .name("snp-autoupdate".into())
                .spawn_scoped(s, worker)
            {
                Ok(_) => spawned = spawned.saturating_add(1),
                Err(e) if spawned == 0 => {
                    return Err(Failure::from_io(&e, None, None));
                }
                Err(_) => break,
            }
        }
        drop(tx);
        let mut first_err: Option<Failure> = None;
        for (i, res) in rx {
            if first_err.is_some() {
                continue;
            }
            let outcome = res.and_then(|u| match todo.get(i) {
                Some(r) => on_result(r, u),
                None => Ok(()),
            });
            if let Err(e) = outcome {
                stop.store(true, Ordering::Release);
                first_err = Some(e);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        frame::interrupted(cx).map_or(Ok(()), Err)
    })
}

/// `pre-commit autoupdate`: migrate-config first, parallel `-j`, rev-line rewrite, freeze comments.
///
/// `autoupdate()` (`autoupdate.py:162-222`): `migrate_config(config, quiet=True)` (may print
/// `Configuration has been migrated.`), `load_config`, the repositories that are neither `local` nor `meta`
/// (`repo: builtin` has no rev either, except under `SNP_COMPAT_STRICT=1`), the 4.6.1 `--repo` check
/// (`repos (…) were not found in {config}`, exit 1), then `_update_one` for every selected repository on
/// `jobs` threads, each result printed in completion order: `[{repo}] updating {old} -> {new}` (`{tag} (frozen)`
/// with `--freeze`), `[{repo}] already up to date!`, or `[{repo}] {error}` (exit 1). When anything changed the rev
/// lines are rewritten in place (CRLF kept, comments kept, `# frozen:` semantics; the PyYAML re-dump when the rev
/// lines cannot be located; `AssertionError: could not find rev lines`, exit 3, X13-CMD-05) and the file replaced
/// atomically.
///
/// One deliberate divergence (D-CRASH-06, X13-GIT-02): a failing git command of one repository (an unreachable
/// remote) is that repository's `[{repo}] <CalledProcessError block>` line with exit 1; the others are still updated
/// and written, where pre-commit aborts everything with exit 3.
pub fn autoupdate(a: &AutoupdateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let f = frame::git_frame(cx, &a.config)?;
    migrate_file(cx, &f.config_path, &f.config_display, f.root(), true)?;
    let regexes = RegexCache::default();
    let ext = frame::ext_mode(cx);
    let cfg = load_config(&f.source(), &frame::load_opts(cx, &regexes, ext))?;
    let config_repos: Vec<ConfigRepo> = cfg
        .repos
        .iter()
        .enumerate()
        .filter_map(|(index, r)| match r {
            RepoConfig::Remote { repo, rev, hooks } => Some(ConfigRepo {
                index,
                repo: Arc::clone(repo),
                rev: Arc::clone(rev),
                hook_ids: hooks.iter().map(|h| Arc::clone(&h.id)).collect(),
            }),
            _ => None,
        })
        .collect();
    let wanted: Vec<String> = a
        .repos
        .iter()
        .map(|r| r.to_string_lossy().into_owned())
        .collect();
    let missing: BTreeSet<&str> = wanted
        .iter()
        .map(String::as_str)
        .filter(|w| !config_repos.iter().any(|r| &*r.repo == *w))
        .collect();
    if !missing.is_empty() {
        let sorted: Vec<&str> = missing.into_iter().collect();
        cx.printer
            .write_line(&messages::repos_not_found(&sorted, &f.config_display));
        return Ok(1);
    }
    let todo: Vec<&ConfigRepo> = config_repos
        .iter()
        .filter(|r| wanted.is_empty() || wanted.iter().any(|w| *w == *r.repo))
        .collect();
    let workers = worker_count(a.jobs, wanted.len(), config_repos.len());
    let mut retv = 0;
    let mut updates: Vec<RevUpdate> = Vec::new();
    run_parallel(cx, &f, &todo, workers, a, &mut |r, u| {
        match u {
            Updated::CannotUpdate(text) => {
                cx.printer.write_line(&messages::repo_error(&r.repo, &text));
                retv = 1;
            }
            Updated::Rev(new) if new.rev != *r.rev => {
                cx.printer.write_line(
                    messages::updating(&r.repo, &r.rev, &new.rev, new.frozen.as_deref()).as_bytes(),
                );
                updates.push(RevUpdate {
                    repo_index: r.index,
                    new_rev: Arc::from(new.rev.as_str()),
                    frozen_comment: new.frozen.as_deref().map(Arc::from),
                });
            }
            Updated::Rev(_) => {
                cx.printer
                    .write_line(messages::up_to_date(&r.repo).as_bytes());
            }
        }
        Ok(())
    })?;
    if !updates.is_empty() {
        updates.sort_by_key(|u| u.repo_index);
        let original = std::fs::read(&f.config_path)
            .map_err(|e| Failure::from_io(&e, Some(&f.config_display), None))?;
        let (Rewrite::Lines(new) | Rewrite::FullReformat(new)) =
            revline::rewrite_with(&original, &updates, ext)?
        else {
            return Err(Failure::Exc {
                kind: snp_core::failure::ExcKind::Unexpected,
                exc: PyExc::new("AssertionError", "could not find rev lines"),
            });
        };
        write_config(&f.config_path, f.root(), &new)?;
    }
    Ok(retv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_formula() {
        assert_eq!(worker_count(1, 0, 5), 1);
        assert_eq!(worker_count(8, 0, 3), 3);
        assert_eq!(worker_count(8, 2, 3), 2);
        assert_eq!(worker_count(-4, 0, 3), 1);
        assert_eq!(worker_count(4, 0, 0), 1);
        assert!(worker_count(0, 0, 1000) >= 1);
        assert_eq!(worker_count(i64::MAX, 0, 7), 7);
    }

    #[test]
    fn failure_text_is_the_exception_text() {
        let f = Failure::unexpected("CalledProcessError", "command: ('git',)\nreturn code: 1");
        assert_eq!(failure_text(&f), b"command: ('git',)\nreturn code: 1");
    }
}
