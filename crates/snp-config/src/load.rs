//! Loading configs and manifests: cfgv `load_from_filename` over snp-yaml (`clientlib.load_config`,
//! `load_manifest`; 01 §2.1, §8.4, §12).
//!
//! `cfgv.load_from_filename(filename, schema, load_strategy, exc_tp)` step by step: `{display} is not a file`
//! (no `File` context) when `os.path.isfile` is false; `open(...).read()` outside the validation context (an
//! unreadable file is an uncaught `OSError`, exit 3, X13-CFG-09); then, inside `File {display}`: strict UTF-8 with
//! universal newlines, `yaml_load` (any error → leaf `str(e)`), `validate`, `apply_defaults`. Warnings flow into
//! `LoadOpts::warn` in cfgv item order; regex `FutureWarning`s go to `LoadOpts::diag` afterwards (D-MISC-04).
//!
//! Owner: WP-21.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use cfgv::{Cx, Schema, ValidationError, Value};
use snp_core::failure::pyerr;
use snp_pyre::Options;

use crate::convert::{self, get, get_str};
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
pub(crate) fn read_file(
    path: &Path,
    display: &str,
    manifest: bool,
) -> Result<Vec<u8>, ConfigError> {
    if !path.is_file() {
        return Err(ConfigError::invalid(
            manifest,
            &ValidationError::new(messages::not_a_file(display)),
        ));
    }
    let os_err = |e: &std::io::Error| ConfigError {
        manifest,
        rendered: format!(
            "{}: {}",
            pyerr::os_error_class(e),
            pyerr::os_error_text(e, Some(OsStr::new(display)), None)
        ),
    };
    let max = limits().max_bytes;
    let mut f = File::open(path).map_err(|e| os_err(&e))?;
    let len = f.metadata().map_or(0, |m| m.len());
    if usize::try_from(len).map_or(true, |l| l > max) {
        // The YAML layer's own limit (D-SEC-11), reported with the file's real size and without reading it.
        return Err(ConfigError::invalid(
            manifest,
            &ValidationError::new(format!(
                "the input is {len} bytes long, more than the limit of {max} bytes"
            ))
            .with_context(format!("File {display}")),
        ));
    }
    let mut bytes = Vec::new();
    let cap = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    (&mut f)
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|e| os_err(&e))?;
    Ok(bytes)
}

/// The steps of `load_from_filename` inside `File {display}`: decode, YAML-load (manifest forward-compat
/// guard), validate with warnings, apply defaults.
pub(crate) fn validate_bytes(
    bytes: &[u8],
    display: &str,
    schema: &Schema,
    kind: Kind,
    o: &LoadOpts<'_>,
) -> Result<Value, ConfigError> {
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
            let mut cx = Cx::new(|msg: String| o.warn.warn(&msg));
            schema.check_with(&data, &mut cx)?;
            Ok(schema.apply_defaults_owned(data))
        },
    );
    result.map_err(|e| ConfigError::invalid(kind.is_manifest(), &e))
}

/// Emits one `[WARNING] regex …` stderr line per distinct (pattern, `FutureWarning`) of the `files` / `exclude`
/// patterns of a validated value, in document order (01 Addendum §A.4, D-MISC-04). Patterns are compiled through
/// the run's cache, so the filters reuse them.
pub(crate) fn future_warnings(value: &Value, kind: Kind, o: &LoadOpts<'_>) {
    fn hook<'a>(h: &'a cfgv::Map, out: &mut Vec<&'a str>) {
        for k in ["files", "exclude"] {
            if let Some(p) = get_str(h, k) {
                out.push(p);
            }
        }
    }
    let mut patterns: Vec<&str> = Vec::new();
    match (kind, value) {
        (Kind::Manifest, Value::List(hooks)) => {
            for h in hooks.iter().filter_map(Value::as_map) {
                hook(h, &mut patterns);
            }
        }
        (_, Value::Map(root)) => {
            hook(root, &mut patterns);
            let repos = get(root, "repos")
                .and_then(Value::as_array)
                .unwrap_or_default();
            for repo in repos.iter().filter_map(Value::as_map) {
                let hooks = get(repo, "hooks")
                    .and_then(Value::as_array)
                    .unwrap_or_default();
                for h in hooks.iter().filter_map(Value::as_map) {
                    hook(h, &mut patterns);
                }
            }
            if let (Kind::Overlay, Some(patches)) =
                (kind, get(root, "hooks").and_then(Value::as_map))
            {
                for h in patches.values().filter_map(Value::as_map) {
                    hook(h, &mut patterns);
                }
            }
        }
        _ => {}
    }
    let mut seen: Vec<(&str, &str)> = Vec::new();
    let mut compiled = Vec::new();
    for p in patterns {
        if p.is_empty() || p == "^$" {
            continue;
        }
        if let Ok(rx) = o.regexes.get(p, Options::CONFIG) {
            compiled.push((p, rx));
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
    future_warnings(&value, Kind::Config, o);
    Ok(convert::config(src.clone(), value, o.ext))
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
    future_warnings(&value, Kind::Manifest, o);
    Ok(convert::manifest(&value))
}
