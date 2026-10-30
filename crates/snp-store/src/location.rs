//! Store location (§5.1, FD-2, ADR-011).
//!
//! pre-commit's `store._get_default_directory` (`store.py:25-36`): `PRE_COMMIT_HOME` or `$XDG_CACHE_HOME/pre-commit`
//! or `expanduser('~/.cache')/pre-commit`, `realpath`'d. snp uses the same shape under its own name and never
//! shares pre-commit's directory (D-STORE-01).
//!
//! Owner: WP-22.

use std::path::{Path, PathBuf};

use snp_core::env::Env;
use snp_core::failure::Failure;

use crate::messages;

/// Which rule chose the store directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreSource {
    /// `SNP_HOME`.
    SnpHome,
    /// `$XDG_CACHE_HOME/snp`.
    XdgCacheHome,
    /// `expanduser("~/.cache")/snp` (Windows `%USERPROFILE%\.cache\snp`).
    PlatformDefault,
}

/// The resolved store directory.
///
/// `store._get_default_directory()` (`store.py:25-36`) under snp's own name.
#[derive(Clone, Debug)]
pub struct StoreLocation {
    /// The store root.
    pub path: PathBuf,
    /// The rule that chose it.
    pub source: StoreSource,
}

/// `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `expanduser("~/.cache")/snp` (Windows `%USERPROFILE%\.cache\snp`); relative
/// values resolved against the invocation cwd; `realpath_nonstrict`'d. `PRE_COMMIT_HOME` is read ONLY to compute
/// pre-commit's store and refuse it (path equality or README signature) with `FatalError` M7 (ADR-011).
///
/// "Non-empty" follows pre-commit's `os.environ.get(...) or` (`store.py:32-35`): an empty value counts as unset.
/// `HOME=""` expands `~/.cache` to `/.cache` exactly as Python's `expanduser` does (15 §A4.9). No I/O beyond the
/// `realpath` walk and one bounded read of `<store>/README`; nothing is created.
///
/// # Errors
///
/// `FatalError` (exit 1) when the result is pre-commit's store (M7) or `~` cannot be expanded; an `OSError` (exit 3)
/// when `realpath` fails (a `readlink` error on an existing link).
pub fn resolve_location(env: &Env, invocation_cwd: &Path) -> Result<StoreLocation, Failure> {
    let (raw, source) = if let Some(home) = env.get_nonempty("SNP_HOME") {
        (PathBuf::from(home), StoreSource::SnpHome)
    } else if let Some(xdg) = env.get_nonempty("XDG_CACHE_HOME") {
        (Path::new(xdg).join("snp"), StoreSource::XdgCacheHome)
    } else {
        match snp_os::home::default_cache_root(env) {
            Some(cache) => (cache.join("snp"), StoreSource::PlatformDefault),
            None => return Err(Failure::fatal("FatalError", messages::NO_HOME)),
        }
    };
    let path = snp_os::realpath::realpath_nonstrict(&raw, invocation_cwd)
        .map_err(|e| Failure::from_io(&e, Some(raw.as_os_str()), None))?;
    if is_pre_commit_store(&path, env, invocation_cwd) {
        return Err(Failure::fatal(
            "FatalError",
            messages::refuse_pre_commit_store(&path),
        ));
    }
    Ok(StoreLocation { path, source })
}

/// pre-commit's own store directory (`store.py:25-36`), realpath'd; `None` when `~` cannot be expanded.
pub(crate) fn pre_commit_store(env: &Env, cwd: &Path) -> Option<PathBuf> {
    let raw = match env.get_nonempty("PRE_COMMIT_HOME") {
        Some(p) => PathBuf::from(p),
        None => snp_os::home::default_cache_root(env)?.join("pre-commit"),
    };
    snp_os::realpath::realpath_nonstrict(&raw, cwd).ok()
}

/// M7: `path` is pre-commit's store, by path equality (`os.path.normcase` on Windows) or by the README signature.
fn is_pre_commit_store(path: &Path, env: &Env, cwd: &Path) -> bool {
    if pre_commit_store(env, cwd).is_some_and(|pc| same_path(&pc, path)) {
        return true;
    }
    readme_first_line(path).is_some_and(|l| l == messages::PRE_COMMIT_README_SIGNATURE)
}

/// Path equality under the platform's `normcase`.
fn same_path(a: &Path, b: &Path) -> bool {
    snp_pycompat::ospath::normcase(a.as_os_str()) == snp_pycompat::ospath::normcase(b.as_os_str())
}

/// The first line of `<dir>/README` (at most 4 KiB read, a trailing `\r` dropped).
pub(crate) fn readme_first_line(dir: &Path) -> Option<Vec<u8>> {
    let bytes = snp_os::fs::read_capped(&dir.join("README"), 4096).ok()?;
    let line = bytes.split(|&b| b == b'\n').next().unwrap_or_default();
    Some(line.strip_suffix(b"\r").unwrap_or(line).to_vec())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    #[test]
    fn precedence_and_relative_values() {
        let cwd = Path::new("/work/proj");
        let e = env(&[("HOME", "/home/u"), ("SNP_HOME", "rel/store")]);
        let loc = resolve_location(&e, cwd).unwrap();
        assert_eq!(loc.path, PathBuf::from("/work/proj/rel/store"));
        assert_eq!(loc.source, StoreSource::SnpHome);

        let e = env(&[
            ("HOME", "/home/u"),
            ("SNP_HOME", ""),
            ("XDG_CACHE_HOME", "/x"),
        ]);
        let loc = resolve_location(&e, cwd).unwrap();
        assert_eq!(loc.path, PathBuf::from("/x/snp"));
        assert_eq!(loc.source, StoreSource::XdgCacheHome);

        let e = env(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "")]);
        let loc = resolve_location(&e, cwd).unwrap();
        assert_eq!(loc.path, PathBuf::from("/home/u/.cache/snp"));
        assert_eq!(loc.source, StoreSource::PlatformDefault);

        // PRE_COMMIT_HOME never moves snp's store.
        let e = env(&[("HOME", "/home/u"), ("PRE_COMMIT_HOME", "/p")]);
        assert_eq!(
            resolve_location(&e, cwd).unwrap().path,
            PathBuf::from("/home/u/.cache/snp")
        );

        // HOME="" → expanduser("~/.cache") == "/.cache" (posixpath).
        let e = env(&[("HOME", "")]);
        assert_eq!(
            resolve_location(&e, cwd).unwrap().path,
            PathBuf::from("/.cache/snp")
        );
    }

    #[test]
    fn refuses_pre_commit_store_by_path() {
        let cwd = Path::new("/w");
        for e in [
            env(&[("HOME", "/h"), ("SNP_HOME", "/h/.cache/pre-commit")]),
            env(&[
                ("HOME", "/h"),
                ("SNP_HOME", "/p"),
                ("PRE_COMMIT_HOME", "/p"),
            ]),
            env(&[
                ("HOME", "/h"),
                ("SNP_HOME", "/x/pre-commit"),
                ("XDG_CACHE_HOME", "/x"),
            ]),
        ] {
            let err = resolve_location(&e, cwd).unwrap_err();
            assert_eq!(err.exit_code(), 1);
            let Failure::Exc { exc, .. } = err else {
                panic!("not an exception")
            };
            assert_eq!(&*exc.ty, "FatalError");
            assert!(
                String::from_utf8_lossy(&exc.text)
                    .starts_with("snp refuses to use pre-commit's cache directory")
            );
        }
    }
}
