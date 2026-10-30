//! Shared helpers of snp-config's integration tests: loading configs from text, the snp-brand substitution of
//! `tests/differential/brand_map.yaml`, the oracle vector files.
//!
//! Owner: WP-21.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cfgv::Value;
use snp_config::{Config, ConfigError, ConfigSource, ExtMode, LoadOpts, Manifest};
use snp_core::brand::Brand;
use snp_core::output::CollectWarnings;
use snp_pyre::RegexCache;

/// Both brands.
pub const BRANDS: [Brand; 2] = [Brand::PreCommit, Brand::Snp];

/// The repository root.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A committed vector file of `tests/vectors/`.
pub fn vectors(name: &str) -> Vec<serde_json::Value> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors").join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    v["cases"].as_array().expect("cases").clone()
}

/// The substring rules of `brand_map.yaml` (COMPATIBILITY §3.3), in order.
fn brand_rules() -> &'static [(String, String)] {
    static RULES: OnceLock<Vec<(String, String)>> = OnceLock::new();
    RULES.get_or_init(|| {
        let p = repo_root().join("tests/differential/brand_map.yaml");
        let text = std::fs::read_to_string(&p).expect("brand_map.yaml");
        let v = snp_yaml::load(&text, &snp_yaml::Limits::default()).expect("brand map yaml");
        let subs = v
            .as_map()
            .and_then(|m| m.get(&Value::from("substitutions")))
            .and_then(Value::as_array)
            .expect("substitutions");
        subs.iter()
            .map(|r| {
                let m = r.as_map().expect("rule");
                let get = |k: &str| m.get(&Value::from(k)).and_then(Value::as_str).expect(k).to_owned();
                (get("from"), get("to"))
            })
            .collect()
    })
}

/// The text `brand` prints for the compat-brand text `s` (identity for the compat brand).
pub fn brand_text(brand: Brand, s: &str) -> String {
    match brand {
        Brand::PreCommit => s.to_owned(),
        Brand::Snp => {
            let mut out = s.to_owned();
            for (from, to) in brand_rules() {
                out = out.replace(from.as_str(), to);
            }
            out
        }
    }
}

/// The result of one load: warnings (stdout `[WARNING]` messages), diagnostics (stderr lines) and the outcome.
#[derive(Debug)]
pub struct Loaded<T> {
    pub warnings: Vec<String>,
    pub diag: Vec<String>,
    pub result: Result<T, ConfigError>,
}

/// Runs `f` with fresh sinks.
pub fn with_opts<T>(brand: Brand, ext: ExtMode, f: impl FnOnce(&LoadOpts<'_>) -> Result<T, ConfigError>) -> Loaded<T> {
    let regexes = RegexCache::new();
    let warn = CollectWarnings::new();
    let diag = CollectWarnings::new();
    let result = {
        let o = LoadOpts {
            brand,
            ext,
            regexes: &regexes,
            warn: &warn,
            diag: &diag,
        };
        f(&o)
    };
    Loaded {
        warnings: warn.take(),
        diag: diag.take(),
        result,
    }
}

/// A temporary directory holding `file` with `bytes` (`None`: the file does not exist).
pub fn file_in_tempdir(file: &str, bytes: Option<&[u8]>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(file);
    if let Some(b) = bytes {
        std::fs::write(&path, b).expect("write");
    }
    (dir, path)
}

/// `load_config` of `text` written to `cfg.yaml` (display `cfg.yaml`).
pub fn load_config_text(text: &str, brand: Brand, ext: ExtMode) -> Loaded<Config> {
    load_config_bytes(text.as_bytes(), "cfg.yaml", brand, ext)
}

/// `load_config` of `bytes` written to `file`.
pub fn load_config_bytes(bytes: &[u8], file: &str, brand: Brand, ext: ExtMode) -> Loaded<Config> {
    let (dir, path) = file_in_tempdir(file, Some(bytes));
    let src = ConfigSource {
        path,
        display: file.into(),
        project_root: dir.path().to_path_buf(),
    };
    with_opts(brand, ext, |o| snp_config::load_config(&src, o))
}

/// `load_manifest_bytes` of `text` (display `m.yaml`).
pub fn load_manifest_text(text: &str, brand: Brand) -> Loaded<Manifest> {
    with_opts(brand, ExtMode::Enabled, |o| {
        snp_config::load_manifest_bytes(text.as_bytes(), OsStr::new("m.yaml"), o)
    })
}

/// The rendered error of a load that must fail.
pub fn err<T>(l: &Loaded<T>) -> String {
    match &l.result {
        Ok(_) => panic!("expected an error; warnings {:?}", l.warnings),
        Err(e) => e.rendered.clone(),
    }
}

/// The config of a load that must succeed.
pub fn ok<T>(l: Loaded<T>) -> (T, Vec<String>) {
    match l.result {
        Ok(v) => (v, l.warnings),
        Err(e) => panic!("unexpected error: {}", e.rendered),
    }
}

/// The context lines of the pre-commit validation chain down to a hook (`\n==> File cfg.yaml\n==> At Config()…`).
pub fn hook_ctx(repo_repr: &str, hook_repr: &str) -> String {
    format!(
        "\n==> File cfg.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo={repo_repr})\n==> At key: hooks\n==> At Hook(id={hook_repr})\n"
    )
}
