//! Loading configs and manifests: cfgv `load_from_filename` over snp-yaml (`clientlib.load_config`,
//! `load_manifest`; 01 §2.1, §8.4, §12).
//!
//! `cfgv.load_from_filename(filename, schema, load_strategy, exc_tp)` step by step: `{display} is not a file`
//! (no `File` context) when `os.path.isfile` is false; `open(...).read()` outside the validation context (an
//! unreadable file is an uncaught `OSError`, exit 3, X13-CFG-09); then, inside `File {display}`: strict UTF-8 with
//! universal newlines, `yaml_load` (any error → leaf `str(e)`), `validate`, `apply_defaults`. Warnings flow into
//! `LoadOpts::warn` in cfgv item order; regex `FutureWarning`s go to `LoadOpts::diag` afterwards, in the order
//! the patterns were compiled and also when validation failed later (D-MISC-04).
//!
//! Owner: WP-21.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use cfgv::{Cx, Schema, ValidationError, Value};
use snp_core::failure::pyerr;
use snp_pyre::Options;

use crate::convert;
use crate::items::COMPILED;
use crate::{Config, ConfigError, ConfigSource, LoadOpts, Manifest, messages, schema};

/// What a load validates.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `.pre-commit-config.yaml` (`InvalidConfigError`).
    Config,
    /// `.pre-commit-hooks.yaml` (`InvalidManifestError`, map-form forward-compat guard).
    Manifest,
    /// The local overlay (reported as `InvalidConfigError`).
    Overlay,
}

impl Kind {
    const fn is_manifest(self) -> bool {
        matches!(self, Self::Manifest)
    }
}

/// The YAML limits of every config load (D-SEC-11).
pub(crate) fn limits() -> snp_yaml::Limits {
    snp_yaml::Limits::default()
}

/// Reads `path` like `cfgv.load_from_filename` up to the validation context: `Err` is the not-a-file validation
/// error or the uncaught `OSError`.
///
/// One `stat(path)` answers both `os.path.isfile` (checked before the open, so a FIFO or device is never opened)
/// and the size limit (checked after the open, so an unreadable file still fails with its `OSError`). The buffer
/// is reserved for the whole file plus one byte, so a small file takes one read for its bytes and one
/// zero-length read instead of std's 32-byte probe-and-double sequence.
pub(crate) fn read_file(
    path: &Path,
    display: &str,
    manifest: bool,
) -> Result<Vec<u8>, ConfigError> {
    let len = match std::fs::metadata(path) {
        Ok(md) if md.is_file() => md.len(),
        _ => {
            return Err(ConfigError::invalid(
                manifest,
                &ValidationError::new(messages::not_a_file(display)),
            ));
        }
    };
    let os_err = |e: &std::io::Error| ConfigError {
        manifest,
        rendered: format!(
            "{}: {}",
            pyerr::os_error_class(e),
            pyerr::os_error_text(e, Some(OsStr::new(display)), None)
        ),
    };
    let max = limits().max_bytes;
    let f = File::open(path).map_err(|e| os_err(&e))?;
    let hint = match usize::try_from(len) {
        Ok(l) if l <= max => l,
        _ => {
            // The YAML layer's own limit (D-SEC-11), reported with the file's size and without reading it.
            return Err(ConfigError::invalid(
                manifest,
                &ValidationError::new(format!(
                    "the input is {len} bytes long, more than the limit of {max} bytes"
                ))
                .with_context(format!("File {display}")),
            ));
        }
    };
    let cap = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    read_reserved(f, hint, cap).map_err(|e| os_err(&e))
}

/// Reads `r` to its end, at most `cap` bytes, into a buffer reserved for `hint` bytes plus the final zero-length
/// read. A reservation that fails (or a hint that overflows) only means the buffer grows as it is filled, as
/// without a hint; a file whose size changed since `hint` was taken still reads to its end (capped).
fn read_reserved(r: impl Read, hint: usize, cap: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(want) = hint.checked_add(1) {
        let _reserved: Result<(), std::collections::TryReserveError> =
            bytes.try_reserve_exact(want);
    }
    r.take(cap).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The steps of `load_from_filename` inside `File {display}`: decode, YAML-load (manifest forward-compat
/// guard), validate with warnings ([`check_bytes`]), then apply the schema defaults (which can neither fail nor
/// warn).
pub(crate) fn validate_bytes(
    bytes: &[u8],
    display: &str,
    schema: &Schema,
    kind: Kind,
    o: &LoadOpts<'_>,
) -> Result<Value, ConfigError> {
    check_bytes(bytes, display, schema, kind, o).map(|checked| defaulted(schema, checked))
}

/// The checking steps of `load_from_filename` inside `File {display}`: decode, YAML-load (manifest forward-compat
/// guard), validate with warnings; `Ok` is the checked value without the schema defaults. Every error and warning
/// of a load comes from here: the defaults and the typed conversion that follow are infallible and silent. The
/// `files` / `exclude` patterns compiled by the checks (`items::Compiled`) print their `FutureWarning`s
/// afterwards, also when a later item failed (pre-commit compiles them during validation, so its warnings reach
/// stderr before the error).
pub(crate) fn check_bytes(
    bytes: &[u8],
    display: &str,
    schema: &Schema,
    kind: Kind,
    o: &LoadOpts<'_>,
) -> Result<Value, ConfigError> {
    let mut compiled: Vec<String> = Vec::new();
    let result = cfgv::validate_context(
        || format!("File {display}"),
        || {
            let text =
                snp_yaml::decode_text(bytes).map_err(|e| ValidationError::new(e.to_string()))?;
            let data = snp_yaml::load(&text, &limits())
                .map_err(|e| ValidationError::new(e.to_string()))?;
            if kind.is_manifest() && matches!(data, Value::Map(_)) {
                // `_load_manifest_forward_compat`: the inner ValidationError is stringified by cfgv.
                let inner = ValidationError::new(messages::min_version(o.brand, "5"));
                return Err(ValidationError::new(inner.to_string()));
            }
            let mut cx = Cx::new(|msg: String| match msg.strip_prefix(COMPILED) {
                Some(p) => compiled.push(p.to_owned()),
                None => o.warn.warn(&msg),
            });
            let checked = schema.check_with(&data, &mut cx);
            drop(cx);
            checked?;
            Ok(data)
        },
    );
    future_warnings(&compiled, kind, o);
    result.map_err(|e| ConfigError::invalid(kind.is_manifest(), &e))
}

/// `apply_defaults` of a checked value (no `Cx`: it cannot fail or warn).
fn defaulted(schema: &Schema, checked: Value) -> Value {
    #[cfg(test)]
    probe::DEFAULTED.with(|n| n.set(n.get().saturating_add(1)));
    schema.apply_defaults_owned(checked)
}

/// Test probes: how often this thread applied the schema defaults (the step check-only loads skip) and compiled a
/// pattern for its `FutureWarning`s.
#[cfg(test)]
pub(crate) mod probe {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static DEFAULTED: Cell<usize> = const { Cell::new(0) };
        pub(crate) static COMPILES: Cell<usize> = const { Cell::new(0) };
    }

    /// The count so far on this thread.
    pub(crate) fn defaulted() -> usize {
        DEFAULTED.with(Cell::get)
    }

    /// How many `FutureWarning` compiles this thread asked the regex cache for.
    pub(crate) fn compiles() -> usize {
        COMPILES.with(Cell::get)
    }
}

/// Emits one `[WARNING] regex …` stderr line per distinct (pattern, `FutureWarning`) of the compiled `files` /
/// `exclude` patterns, in the order the schema compiled them (cfgv item order: a config's hooks before its
/// top-level `files` / `exclude`; 01 Addendum §A.4, D-MISC-04). A pattern prints its warnings only when its compile
/// succeeds.
///
/// A config's patterns are compiled through the run's cache, so the filters reuse them (the config load runs beside
/// discovery, off the critical path). A manifest's patterns are first parsed with [`snp_pyre::validate`], the same
/// parse under the same flags that [`Options::CONFIG`]'s compile starts with, so it returns the same warnings: only a
/// pattern with warnings is compiled (to learn whether the compile succeeds), and the others are left to the filters
/// of the hooks that use them. An empty list prints nothing whatever the compile would do, and a
/// parse error means the compile fails too.
fn future_warnings(patterns: &[String], kind: Kind, o: &LoadOpts<'_>) {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    let mut compiled = Vec::new();
    for p in patterns {
        if p.is_empty() || p == "^$" {
            continue;
        }
        if kind.is_manifest()
            && snp_pyre::validate(p, Options::CONFIG.mode).map_or(true, |w| w.is_empty())
        {
            continue;
        }
        #[cfg(test)]
        probe::COMPILES.with(|n| n.set(n.get().saturating_add(1)));
        if let Ok(rx) = o.regexes.get(p, Options::CONFIG) {
            compiled.push((p.as_str(), rx));
        }
    }
    for (p, rx) in &compiled {
        for w in rx.warnings() {
            if seen.contains(&(*p, &*w.msg)) {
                continue;
            }
            seen.push((p, &w.msg));
            o.diag.diag(&messages::future_warning(
                &snp_pycompat::repr::repr_str(p),
                &w.msg,
            ));
        }
    }
}

/// `display_filename or filename` (`cfgv.py:430`), lossily decoded for cfgv's `&str` display name.
pub(crate) fn display_name<'a>(display: &'a OsStr, path: &'a Path) -> std::borrow::Cow<'a, str> {
    if display.is_empty() {
        path.to_string_lossy()
    } else {
        display.to_string_lossy()
    }
}

/// `load_config` (`clientlib.py`): read, decode, PyYAML-load, validate, default, transform (01 §2–§5).
///
/// `src.display` (the `-c` path as pre-commit shows it, relative to the repository root for run-type commands,
/// 01 §8.4) is converted with `to_string_lossy` for cfgv's `load_from_filename_with(…, display_filename:
/// Option<&str>, …)`.
pub fn load_config(src: &ConfigSource, o: &LoadOpts<'_>) -> Result<Config, ConfigError> {
    let display = display_name(&src.display, &src.path);
    let bytes = read_file(&src.path, &display, false)?;
    let schema = schema::config_schema(o.brand, o.ext);
    let value = validate_bytes(&bytes, &display, schema, Kind::Config, o)?;
    Ok(convert::config(src.clone(), value, o.ext))
}

/// The checking half of [`load_config`]: read, decode, PyYAML-load and validate with warnings, without the schema
/// defaults and the typed conversion. `Ok` is the checked, un-defaulted value; it fails, warns and prints regex
/// `FutureWarning`s exactly when [`load_config`] does (non-strict `validate-config`; loads that read only a few
/// raw keys).
pub fn check_config(src: &ConfigSource, o: &LoadOpts<'_>) -> Result<Value, ConfigError> {
    let display = display_name(&src.display, &src.path);
    let bytes = read_file(&src.path, &display, false)?;
    check_bytes(
        &bytes,
        &display,
        schema::config_schema(o.brand, o.ext),
        Kind::Config,
        o,
    )
}

/// The checking half of [`load_manifest`] (see [`check_config`]): `Ok` is the checked, un-defaulted value.
pub fn check_manifest(
    path: &Path,
    display: &OsStr,
    o: &LoadOpts<'_>,
) -> Result<Value, ConfigError> {
    let display = display_name(display, path);
    let bytes = read_file(path, &display, true)?;
    check_bytes(
        &bytes,
        &display,
        schema::manifest_schema(o.brand),
        Kind::Manifest,
        o,
    )
}

/// The checking half of [`load_manifest_bytes`] (see [`check_config`]): `Ok` is the checked, un-defaulted value.
pub fn check_manifest_bytes(
    bytes: &[u8],
    display: &OsStr,
    o: &LoadOpts<'_>,
) -> Result<Value, ConfigError> {
    check_bytes(
        bytes,
        &display.to_string_lossy(),
        schema::manifest_schema(o.brand),
        Kind::Manifest,
        o,
    )
}

/// `load_manifest` (`clientlib.py`). `display` is converted with `to_string_lossy` for cfgv's
/// `load_from_filename_with(…, display_filename: Option<&str>, …)`.
pub fn load_manifest(
    path: &Path,
    display: &OsStr,
    o: &LoadOpts<'_>,
) -> Result<Manifest, ConfigError> {
    let display = display_name(display, path);
    let bytes = read_file(path, &display, true)?;
    manifest_from_bytes(&bytes, &display, o)
}

/// `load_manifest` over embedded bytes (Tier A per-tag manifests, builtin manifest): the steps inside
/// `File {display}`.
pub fn load_manifest_bytes(
    bytes: &[u8],
    display: &OsStr,
    o: &LoadOpts<'_>,
) -> Result<Manifest, ConfigError> {
    manifest_from_bytes(bytes, &display.to_string_lossy(), o)
}

fn manifest_from_bytes(
    bytes: &[u8],
    display: &str,
    o: &LoadOpts<'_>,
) -> Result<Manifest, ConfigError> {
    let value = validate_bytes(
        bytes,
        display,
        schema::manifest_schema(o.brand),
        Kind::Manifest,
        o,
    )?;
    Ok(convert::manifest(&value))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use proptest::prelude::*;
    use snp_core::brand::Brand;
    use snp_core::output::CollectWarnings;
    use snp_pyre::{Options, RegexCache};

    use super::{Kind, future_warnings, limits, probe, read_file, read_reserved};
    use crate::{ExtMode, LoadOpts, messages};

    /// Runs `f` with fresh sinks and cache: (its result, the diagnostics, the cache). Nothing here warns.
    fn with_opts<T>(f: impl FnOnce(&LoadOpts<'_>) -> T) -> (T, Vec<String>, RegexCache) {
        let regexes = RegexCache::new();
        let warn = CollectWarnings::new();
        let diag = CollectWarnings::new();
        let out = f(&LoadOpts {
            brand: Brand::Snp,
            ext: ExtMode::Enabled,
            regexes: &regexes,
            warn: &warn,
            diag: &diag,
        });
        assert_eq!(warn.take(), Vec::<String>::new());
        (out, diag.take(), regexes)
    }

    /// The `FutureWarning` lines of the old rule: compile every pattern (skipping `''` and `^$`), print the
    /// warnings of the compiles that succeed, one line per distinct (pattern, message), in pattern order.
    fn compile_all(patterns: &[String]) -> Vec<String> {
        let cache = RegexCache::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut out = Vec::new();
        for p in patterns.iter().filter(|p| !p.is_empty() && *p != "^$") {
            let Ok(rx) = cache.get(p, Options::CONFIG) else {
                continue;
            };
            for w in rx.warnings() {
                let key = (p.clone(), w.msg.to_string());
                if !seen.contains(&key) {
                    out.push(messages::future_warning(
                        &snp_pycompat::repr::repr_str(p),
                        &w.msg,
                    ));
                    seen.push(key);
                }
            }
        }
        out
    }

    /// Pattern fragments: plain, with `FutureWarning`s (nested sets, set operations), broken, and over the engine
    /// budget (parses, warns, fails to compile).
    const FRAGMENTS: [&str; 15] = [
        "a", "^src/", r"\.py$", "[[a]", "[a--b]", "[a&&b]", "[a||b]", "[a~~b]", "(", "[", "(?i)",
        r"\d", "{100000}", "|", "^$",
    ];

    proptest! {
        /// A manifest's `FutureWarning` lines (parse first, compile only the patterns with warnings) equal the
        /// lines of compiling every pattern, for any list of patterns; a config still compiles every pattern.
        #[test]
        fn manifest_warnings_equal_compiling_everything(
            patterns in prop::collection::vec(
                prop::collection::vec(prop::sample::select(FRAGMENTS.to_vec()), 0..4)
                    .prop_map(|f| f.concat()),
                0..6,
            )
        ) {
            let want = compile_all(&patterns);
            for kind in [Kind::Manifest, Kind::Config] {
                let ((), got, _) = with_opts(|o| future_warnings(&patterns, kind, o));
                prop_assert_eq!(&got, &want, "{:?}", kind);
            }
        }
    }

    /// A manifest compiles only the patterns that warn: a pattern without warnings is never compiled (the filters
    /// of the hooks that use it compile it later), a warning pattern still warns once, and a warning pattern
    /// whose compile fails (over the engine budget) stays silent.
    #[test]
    fn manifest_compiles_only_warning_patterns() {
        let hook = |id: &str, filters: &str| {
            format!(
                "-   id: {id}\n    name: {id}\n    entry: {id}\n    language: system\n{filters}"
            )
        };
        let yaml = [
            hook("a", "    files: '^src/.*\\.py$'\n"),
            hook("b", "    files: '[[a]'\n    exclude: '[[a]{100000}'\n"),
            hook("c", "    files: '[[a]'\n"),
        ]
        .concat();
        let before = probe::compiles();
        let (m, diag, _) =
            with_opts(|o| super::load_manifest_bytes(yaml.as_bytes(), OsStr::new("m.yaml"), o));
        assert_eq!(m.unwrap().hooks.len(), 3);
        assert_eq!(
            diag,
            [messages::future_warning(
                "'[[a]'",
                "Possible nested set at position 1"
            )]
        );
        assert_eq!(
            diag,
            compile_all(&["^src/.*\\.py$", "[[a]", "[[a]{100000}", "[[a]"].map(str::to_owned))
        );
        // '[[a]' (twice, the second a cache hit) and '[[a]{100000}': never the plain pattern.
        assert_eq!(probe::compiles() - before, 3);
    }

    /// A pattern that fails to parse is not compiled for a manifest (the compile would fail the same way); a config
    /// compiles it (and caches the error).
    #[test]
    fn manifest_parse_error_is_not_compiled() {
        let broken = ["(".to_owned()];
        let before = probe::compiles();
        let ((), diag, _) = with_opts(|o| future_warnings(&broken, Kind::Manifest, o));
        assert_eq!(diag, Vec::<String>::new());
        assert_eq!(probe::compiles() - before, 0);
        let ((), diag, _) = with_opts(|o| future_warnings(&broken, Kind::Config, o));
        assert_eq!(diag, Vec::<String>::new());
        assert_eq!(probe::compiles() - before, 1);
    }

    /// The reserved read returns the same bytes at the probe boundaries, the exact limit and above it, with the
    /// buffer sized once (capacity = length + 1); one byte over the limit is refused without reading.
    #[test]
    fn reserved_read_matches_and_sizes_once() {
        let dir = tempfile::tempdir().unwrap();
        let max = limits().max_bytes;
        for len in [0usize, 1, 31, 32, 33, 64, 1484, 8192, 8193, 70_000, max] {
            let bytes: Vec<u8> = (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect();
            let p = dir.path().join(format!("f{len}"));
            std::fs::write(&p, &bytes).unwrap();
            let got = read_file(&p, "f", false).unwrap();
            assert_eq!(got, bytes, "{len}");
            assert_eq!(got.capacity(), len + 1, "{len}");
        }
        let over = dir.path().join("over");
        std::fs::write(&over, vec![b'a'; max + 1]).unwrap();
        let e = read_file(&over, "over", true).unwrap_err();
        assert!(e.manifest);
        assert_eq!(
            e.rendered,
            format!(
                "\n==> File over\n=====> the input is {} bytes long, more than the limit of {max} bytes",
                max + 1
            )
        );
    }

    /// A reservation that cannot be made (or a stale hint) only changes how the buffer grows: the bytes are the
    /// same, still capped.
    #[test]
    fn failed_or_stale_reservation_falls_back() {
        let data: Vec<u8> = (0..5000u32).map(|i| u8::try_from(i % 7).unwrap()).collect();
        for hint in [usize::MAX, usize::MAX / 2, 0, 3, 4999, 5000, 5001, 100_000] {
            let got = read_reserved(&data[..], hint, 1 << 20).unwrap();
            assert_eq!(got, data, "{hint}");
        }
        let capped = read_reserved(&data[..], usize::MAX / 2, 100).unwrap();
        assert_eq!(capped, &data[..100]);
    }

    /// `os.path.isfile` stays first: a directory or a missing path is "not a file", never opened.
    #[test]
    fn not_a_file_before_open() {
        let dir = tempfile::tempdir().unwrap();
        for p in [dir.path().to_path_buf(), dir.path().join("missing")] {
            let e = read_file(&p, "x.yaml", false).unwrap_err();
            assert_eq!(e.rendered, "\n=====> x.yaml is not a file");
        }
    }

    /// The open comes before the size limit: an unreadable over-limit file is still the uncaught `OSError`
    /// pre-commit's `open()` raises (skipped when running as root, which can open it).
    #[cfg(unix)]
    #[test]
    fn open_error_before_size_limit() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.yaml");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(u64::try_from(limits().max_bytes).unwrap() + 1)
            .unwrap();
        drop(f);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&p).is_ok() {
            return;
        }
        let e = read_file(&p, "big.yaml", false).unwrap_err();
        assert!(
            e.rendered
                .starts_with("PermissionError: [Errno 13] Permission denied: "),
            "{}",
            e.rendered
        );
    }
}
