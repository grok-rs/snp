//! `validate-config` / `validate-manifest` (`commands/validate_config.py`, `validate_manifest.py`; 01 §8.1):
//! per-file `print(e)` blocks, no git.
//!
//! Owner: WP-21.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use crate::convert::extra_keys;
use crate::items::{BUILTIN, LOCAL, META};
use crate::{Config, ConfigError, ConfigSource, LoadOpts, RepoConfig, messages};

/// Which schema a file is validated against.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum FileKind {
    /// `validate-config`.
    Config,
    /// `validate-manifest`.
    Manifest,
}

/// `(hook id, unknown keys)` of one repo.
type HookKeys<'a> = Vec<(&'a str, Vec<Arc<str>>)>;

/// `--strict`: the W11 warnings `run` would print when it builds the hooks, emitted at validation time (config-level
/// keys only: a remote hook's manifest is not read).
fn strict_warnings(cfg: &Config, o: &LoadOpts<'_>) {
    for repo in &cfg.repos {
        let (src, hooks): (&str, HookKeys<'_>) = match repo {
            RepoConfig::Remote { repo, hooks, .. } => (
                repo,
                hooks
                    .iter()
                    .map(|h| (&*h.id, extra_keys(&h.raw, o.ext)))
                    .collect(),
            ),
            RepoConfig::Builtin { hooks } => (
                BUILTIN,
                hooks
                    .iter()
                    .map(|h| (&*h.id, extra_keys(&h.raw, o.ext)))
                    .collect(),
            ),
            RepoConfig::Local { hooks } => (
                LOCAL,
                hooks
                    .iter()
                    .map(|h| (&*h.id, h.extra_keys.clone()))
                    .collect(),
            ),
            RepoConfig::Meta { hooks } => (
                META,
                hooks
                    .iter()
                    .map(|h| (&*h.id, h.extra_keys.clone()))
                    .collect(),
            ),
        };
        for (id, extra) in hooks {
            if !extra.is_empty() {
                o.warn.warn(&messages::unexpected_keys_hook(
                    src,
                    id,
                    &messages::join(&extra),
                ));
            }
        }
    }
}

/// Validates one file (`load_config(filename)` / `load_manifest(filename)`); warnings go to `o.warn` as they are
/// found. `strict` (snp brand `validate-config --strict`) additionally reports the hook keys pre-commit only
/// reports when it builds the hooks (W11, `Unexpected key(s) present on {repo} => {id}: …`), at validation time.
///
/// `Err` is the `InvalidConfigError` / `InvalidManifestError`; an [uncaught](ConfigError::uncaught) error (an
/// unreadable file, X13-CFG-09) aborts the command with exit 3 in pre-commit, later files unprocessed.
pub fn validate(
    kind: FileKind,
    path: &OsStr,
    o: &LoadOpts<'_>,
    strict: bool,
) -> Result<(), ConfigError> {
    match kind {
        FileKind::Config => {
            let p = Path::new(path);
            let src = ConfigSource {
                path: p.to_path_buf(),
                display: path.to_os_string(),
                project_root: p.parent().map_or_default(Path::to_path_buf),
            };
            if strict {
                let cfg = crate::load_config(&src, o)?;
                strict_warnings(&cfg, o);
            } else {
                // Errors and warnings all come from the check; the model would only be dropped.
                crate::check_config(&src, o)?;
            }
            Ok(())
        }
        FileKind::Manifest => crate::check_manifest(Path::new(path), path, o).map(|_| ()),
    }
}

/// Validates one file; `Some(text)` = the `print(e)` block of a failure (`str(e)`, without the newline `print`
/// adds). The text of an [uncaught](ConfigError::uncaught) error does not start with `\n`: it is the
/// `{type}: {message}` of an exit-3 crash and the caller stops (13 X.4.1 CFG-09).
pub fn validate_file(
    kind: FileKind,
    path: &OsStr,
    o: &LoadOpts<'_>,
    strict: bool,
) -> Option<String> {
    validate(kind, path, o, strict).err().map(|e| e.rendered)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use snp_core::brand::Brand;
    use snp_core::output::CollectWarnings;
    use snp_pyre::RegexCache;

    use super::{FileKind, validate};
    use crate::load::probe;
    use crate::{ExtMode, LoadOpts};

    /// Non-strict validation stops after the check (no schema defaults, no model); `--strict` builds the model.
    #[test]
    fn only_strict_validation_applies_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("cfg.yaml");
        std::fs::write(
            &cfg,
            "repos:\n-   repo: local\n    hooks:\n    -   id: h\n        name: h\n        entry: h\n        language: system\n        bogus: 1\n",
        )
        .unwrap();
        let manifest = dir.path().join("m.yaml");
        std::fs::write(
            &manifest,
            "-   id: h\n    name: h\n    entry: h\n    language: system\n",
        )
        .unwrap();
        let regexes = RegexCache::new();
        let warn = CollectWarnings::new();
        let diag = CollectWarnings::new();
        let o = LoadOpts {
            brand: Brand::Snp,
            ext: ExtMode::Enabled,
            regexes: &regexes,
            warn: &warn,
            diag: &diag,
        };
        let before = probe::defaulted();
        validate(FileKind::Config, cfg.as_os_str(), &o, false).unwrap();
        validate(FileKind::Manifest, manifest.as_os_str(), &o, false).unwrap();
        validate(FileKind::Manifest, manifest.as_os_str(), &o, true).unwrap();
        assert_eq!(probe::defaulted(), before);
        assert_eq!(warn.take(), Vec::<String>::new());
        validate(FileKind::Config, cfg.as_os_str(), &o, true).unwrap();
        assert_eq!(probe::defaulted(), before + 1);
        // The W11 hook-key warning exists only on the strict path, which needs the model.
        assert_eq!(warn.take().len(), 1);
        let missing = validate(FileKind::Config, OsStr::new("no-such.yaml"), &o, false);
        assert_eq!(
            missing.map_err(|e| e.rendered),
            Err("\n=====> no-such.yaml is not a file".to_owned())
        );
        assert_eq!(probe::defaulted(), before + 1);
    }
}
