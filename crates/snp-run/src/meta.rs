//! The meta hooks, in process (ARCHITECTURE §3.11; 01 §9; `meta_hooks/check_hooks_apply.py`,
//! `check_useless_excludes.py`, `identity.py`).
//!
//! Each reproduces the bytes of its `python -m pre_commit.meta_hooks.*` subprocess: argv = `args + filenames`, all of
//! them config paths, defaulting to `.pre-commit-config.yaml` (never the `-c` value: the quirk is kept); warnings of
//! the config reload and of `all_hooks` are printed **bare** on the subprocess' line-buffered stderr (no logging
//! handler: `logging.lastResort`), ahead of the block-buffered stdout prints; INFO records are dropped.
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_config::{Config, ConfigHook, RepoConfig};
use snp_core::env::Env;
use snp_core::failure::{Failure, PyExc};
use snp_core::lang::LanguageId;
use snp_core::output::CollectWarnings;
use snp_files::{Classifier, FileTable, HookFilter};
use snp_git::Git;
use snp_hooks::framework::PyOut;
use snp_pyre::Options;
use snp_repo::{MetaHook, ResolveCtx};
use snp_store::Store;

use crate::RunCtx;
use crate::common::{self, ResolveParts};

/// What the meta hooks need of the run.
#[derive(Debug)]
pub(crate) struct MetaCx<'a> {
    /// The run context.
    pub(crate) cx: &'a RunCtx,
    /// The user repository (`run_env`).
    pub(crate) git: &'a Git,
    /// `run_env`.
    pub(crate) env: &'a Env,
    /// `Store()` of the subprocess: the user's store.
    pub(crate) store: &'a Store,
    /// The run's resolution context (fast-path policy).
    pub(crate) rcx: &'a ResolveCtx<'a>,
}

/// The default config path of the meta hooks' argparse (`C.CONFIG_FILE`).
const CONFIG_FILE: &str = ".pre-commit-config.yaml";

/// Runs one meta hook on `args + files`; `(exit code, merged output)`.
///
/// # Errors
///
/// `Failure::Interrupted` on cancellation (any other error is the subprocess' traceback in the output).
pub(crate) fn run(
    m: &MetaCx<'_>,
    which: MetaHook,
    args: &[Arc<str>],
    files: &[OsString],
) -> Result<(i32, Vec<u8>), Failure> {
    let mut argv: Vec<OsString> = args.iter().map(|a| OsString::from(&**a)).collect();
    argv.extend(files.iter().cloned());
    let mut out = PyOut::default();
    let code = match which {
        MetaHook::Identity => {
            for a in &argv {
                out.print_bytes_text(a.as_encoded_bytes());
            }
            0
        }
        MetaHook::CheckHooksApply => each_config(m, &argv, &mut out, check_hooks_apply)?,
        MetaHook::CheckUselessExcludes => each_config(m, &argv, &mut out, check_useless_excludes)?,
        _ => 0,
    };
    Ok((code, out.finish()))
}

/// The per-file result of a check.
type Check = fn(&MetaCx<'_>, &Config, &mut PyOut) -> Result<bool, Failure>;

/// `for filename in args.filenames: retv |= check(filename)`; a traceback ends the subprocess with exit 1.
fn each_config(
    m: &MetaCx<'_>,
    argv: &[OsString],
    out: &mut PyOut,
    check: Check,
) -> Result<i32, Failure> {
    let default = [OsString::from(CONFIG_FILE)];
    let paths = if argv.is_empty() { &default[..] } else { argv };
    let mut retv = false;
    for p in paths {
        match load(m, p, out).and_then(|cfg| check(m, &cfg, out)) {
            Ok(failed) => retv |= failed,
            Err(f @ (Failure::Interrupted(_) | Failure::StdoutClosed)) => return Err(f),
            Err(f) => {
                out.traceback(&traceback_exc(f));
                return Ok(1);
            }
        }
    }
    Ok(i32::from(retv))
}

/// The exception a failure stands for in the subprocess' traceback.
fn traceback_exc(f: Failure) -> PyExc {
    match f {
        Failure::Exc { exc, .. } => qualify(exc),
        other => snp_hooks::framework::failure_exc(other),
    }
}

/// pre-commit's exception classes print with their module in a traceback.
fn qualify(mut exc: PyExc) -> PyExc {
    let module = match &*exc.ty {
        "InvalidConfigError" | "InvalidManifestError" => Some("pre_commit.clientlib"),
        "FatalError" => Some("pre_commit.errors"),
        "CalledProcessError" => Some("pre_commit.util"),
        _ => None,
    };
    if let Some(module) = module {
        exc.ty = format!("{module}.{}", exc.ty).into();
    }
    exc
}

/// Replays collected warnings bare on the subprocess' stderr.
fn replay(c: &CollectWarnings, out: &mut PyOut) {
    for w in c.take() {
        out.eprint(&w);
    }
}

/// `load_config(filename)` relative to the root (the subprocess' cwd), warnings bare.
fn load(m: &MetaCx<'_>, path: &OsStr, out: &mut PyOut) -> Result<Config, Failure> {
    let root = m.git.root();
    let src = common::config_source(path, root);
    let warn = CollectWarnings::new();
    let r = common::load_config_with(m.cx, &src, &warn, &warn);
    replay(&warn, out);
    r
}

/// `Classifier.from_config(git ls-files, config.files, config.exclude)` and the tracked list itself.
fn classifier(m: &MetaCx<'_>, cfg: &Config) -> Result<(Classifier, Arc<FileTable>), Failure> {
    let root = m.git.root();
    let (buf, spans) = m.git.all_files()?.into_parts();
    let all = Arc::new(FileTable::from_zbuf(buf.clone(), spans.clone(), root));
    let table = Arc::new(FileTable::from_zbuf(buf, spans, root));
    let include = regex(m, &cfg.files)?;
    let exclude = regex(m, &cfg.exclude)?;
    let c = Classifier::new(table, &include, &exclude, &m.cx.pool)?;
    Ok((c, all))
}

fn regex(m: &MetaCx<'_>, pattern: &str) -> Result<Arc<snp_pyre::PyRegex>, Failure> {
    m.cx.regexes
        .get(pattern, Options::CONFIG)
        .map_err(|e| Failure::unexpected("re.error", e.to_string()))
}

/// `check_hooks_apply.check_all_hooks_match_files`: every hook that is not `always_run` and not `language: fail`
/// must match at least one tracked file.
fn check_hooks_apply(m: &MetaCx<'_>, cfg: &Config, out: &mut PyOut) -> Result<bool, Failure> {
    let (c, _) = classifier(m, cfg)?;
    let warn = CollectWarnings::new();
    let printer = common::null_printer(m.cx);
    let parts = ResolveParts::new(m.env.clone(), m.git.root().to_path_buf(), m.store);
    let policy = m.rcx.fast_path.clone();
    let rcx = parts.ctx(m.cx, cfg, &printer, &warn, policy, None, &m.cx.cancel);
    let hooks = m.cx.resolver.all_hooks(&rcx);
    replay(&warn, out);
    let hooks = hooks?;
    let mut retv = false;
    for h in &hooks {
        if h.def.always_run || h.def.language == LanguageId::Fail {
            continue;
        }
        let f = HookFilter::compile(
            &h.def.files,
            &h.def.exclude,
            h.def.types,
            h.def.types_or,
            h.def.exclude_types,
            None,
            &m.cx.regexes,
        )
        .map_err(|e| Failure::unexpected("re.error", e.to_string()))?;
        if c.for_hook(&f, &m.cx.pool)?.is_empty() {
            out.print(&format!("{} does not apply to this repository", h.def.id));
            retv = true;
        }
    }
    Ok(retv)
}

/// A filter over plain tag names (config-level dicts).
fn filter_names(
    m: &MetaCx<'_>,
    files: &str,
    exclude: &str,
    h: &RawHook,
) -> Result<HookFilter, Failure> {
    HookFilter::compile(
        files,
        exclude,
        tags(&h.types)?,
        tags(&h.types_or)?,
        tags(&h.exclude_types)?,
        None,
        &m.cx.regexes,
    )
    .map_err(|e| Failure::unexpected("re.error", e.to_string()))
}

/// identify tag names as the filter's tag set (validated by the config schema already).
fn tags<T: for<'a> TryFrom<&'a [&'a str]>>(names: &[String]) -> Result<T, Failure> {
    let v: Vec<&str> = names.iter().map(String::as_str).collect();
    T::try_from(v.as_slice())
        .map_err(|_| Failure::unexpected("ValueError", format!("unknown tag in {names:?}")))
}

/// `check_useless_excludes.check_useless_excludes`.
fn check_useless_excludes(m: &MetaCx<'_>, cfg: &Config, out: &mut PyOut) -> Result<bool, Failure> {
    let (c, all) = classifier(m, cfg)?;
    let mut retv = false;
    let all_ids: Vec<snp_files::FileId> = (0..all.len())
        .filter_map(|i| u32::try_from(i).ok().map(snp_files::FileId))
        .collect();
    if !exclude_matches_any(m, &all, &all_ids, "", &cfg.exclude)? {
        out.print(&format!(
            "The global exclude pattern {} does not match any files",
            snp_pycompat::repr::repr_str(&cfg.exclude)
        ));
        retv = true;
    }
    for repo in &cfg.repos {
        for hook in config_hooks(repo) {
            let f = filter_names(m, "", "^$", &hook)?;
            let names = c.by_types(&c.kept, &f)?;
            if !exclude_matches_any(m, &c.table, &names, &hook.files, &hook.exclude)? {
                out.print(&format!(
                    "The exclude pattern {} for {} does not match any files",
                    snp_pycompat::repr::repr_str(&hook.exclude),
                    hook.id
                ));
                retv = true;
            }
        }
    }
    Ok(retv)
}

/// A config-level hook dict with `types` defaulting to `[]` and the other keys to the manifest defaults.
#[derive(Debug)]
struct RawHook {
    id: Arc<str>,
    files: Arc<str>,
    exclude: Arc<str>,
    types: Vec<String>,
    types_or: Vec<String>,
    exclude_types: Vec<String>,
}

/// The hooks of a repo as `check_useless_excludes` reads them: remote and builtin hooks unmerged (the config's own
/// keys only), local and meta hooks with their schema defaults.
fn config_hooks(repo: &RepoConfig) -> Vec<RawHook> {
    match repo {
        RepoConfig::Remote { hooks, .. } | RepoConfig::Builtin { hooks } => {
            hooks.iter().map(raw_hook).collect()
        }
        RepoConfig::Local { hooks } | RepoConfig::Meta { hooks } => hooks
            .iter()
            .map(|h| RawHook {
                id: Arc::clone(&h.id),
                files: Arc::clone(&h.files),
                exclude: Arc::clone(&h.exclude),
                types: h.types.names().map(str::to_owned).collect(),
                types_or: h.types_or.names().map(str::to_owned).collect(),
                exclude_types: h.exclude_types.names().map(str::to_owned).collect(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn raw_hook(h: &ConfigHook) -> RawHook {
    let get = |key: &str| {
        h.raw
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v)
    };
    let s = |key: &str, default: &str| -> Arc<str> {
        Arc::from(get(key).and_then(|v| v.as_str()).unwrap_or(default))
    };
    let tags = |key: &str| -> Vec<String> {
        get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    RawHook {
        id: Arc::clone(&h.id),
        files: s("files", ""),
        exclude: s("exclude", "^$"),
        types: tags("types"),
        types_or: tags("types_or"),
        exclude_types: tags("exclude_types"),
    }
}

/// `exclude_matches_any(filenames, include, exclude)`: the `'^$'` short-cut is a string comparison.
fn exclude_matches_any(
    m: &MetaCx<'_>,
    table: &FileTable,
    ids: &[snp_files::FileId],
    include: &str,
    exclude: &str,
) -> Result<bool, Failure> {
    if exclude == "^$" {
        return Ok(true);
    }
    let inc = regex(m, include)?;
    let exc = regex(m, exclude)?;
    for &id in ids {
        let name = table.match_view(id);
        if search(&inc, name, exclude)? && search(&exc, name, exclude)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn search(r: &snp_pyre::PyRegex, name: &str, pattern: &str) -> Result<bool, Failure> {
    r.search(name).map_err(|_| {
        Failure::fatal(
            "FatalError",
            format!(
                "snp: regular expression {} exceeded the backtracking limit on {}",
                snp_pycompat::repr::repr_str(pattern),
                snp_pycompat::repr::repr_str(name)
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use snp_hooks::framework::crash_line;
    use std::path::Path;

    #[test]
    fn qualified_tracebacks() {
        let e = qualify(PyExc::new("InvalidConfigError", "\n=====> x"));
        assert_eq!(
            crash_line(&e),
            b"pre_commit.clientlib.InvalidConfigError: \n=====> x\n"
        );
        let e = qualify(PyExc::new("ValueError", "v"));
        assert_eq!(crash_line(&e), b"ValueError: v\n");
    }

    #[test]
    fn root_default_config() {
        assert_eq!(Path::new(CONFIG_FILE).extension().unwrap(), "yaml");
    }
}
