//! Fuzz target `config_manifest_load`: `.pre-commit-hooks.yaml` load + validation.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! `load_manifest_bytes` in both brands; every hook of a valid manifest is merged with an empty config hook and with
//! a config hook overriding several keys. A failure is an `InvalidManifestError` rendered with its `==> File`
//! context (or an uncaught `ValueError` for an integer beyond CPython's digit limit).
//!
//! Owner: WP-21.
#![no_main]

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use snp_config::{Config, ConfigSource, ExtMode, LoadOpts, RepoConfig, merge_hook};
use snp_core::brand::Brand;
use snp_core::output::CollectWarnings;
use snp_pyre::RegexCache;

const CONFIG: &str = "default_language_version: {python: python3}\ndefault_stages: [pre-commit]\nrepos:\n\
-   repo: https://example.com/r\n    rev: v1.0.0\n    hooks:\n    -   id: x\n    -   id: y\n        args: [--a]\n        \
stages: [commit]\n        language: system\n        priority: 3\n";

fn config(o: &LoadOpts<'_>) -> Option<&'static Config> {
    static CFG: OnceLock<Option<Config>> = OnceLock::new();
    CFG.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("snp-fuzz-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&d).ok()?;
        let p: PathBuf = d.join("cfg.yaml");
        std::fs::write(&p, CONFIG).ok()?;
        let src = ConfigSource {
            path: p,
            display: "cfg.yaml".into(),
            project_root: d,
        };
        snp_config::load_config(&src, o).ok()
    })
    .as_ref()
}

fuzz_target!(|data: &[u8]| {
    for brand in [Brand::PreCommit, Brand::Snp] {
        let regexes = RegexCache::new();
        let sink = CollectWarnings::new();
        let o = LoadOpts {
            brand,
            ext: ExtMode::Enabled,
            regexes: &regexes,
            warn: &sink,
            diag: &sink,
        };
        match snp_config::load_manifest_bytes(data, OsStr::new("m.yaml"), &o) {
            Err(e) => {
                assert!(e.manifest);
                if e.uncaught().is_none() {
                    assert!(
                        e.rendered.starts_with("\n==> File m.yaml\n"),
                        "{:?}",
                        e.rendered
                    );
                }
            }
            Ok(manifest) => {
                let Some(cfg) = config(&o) else { return };
                let Some(RepoConfig::Remote { hooks, .. }) = cfg.repos.first() else {
                    return;
                };
                for m in &manifest.hooks {
                    for c in hooks {
                        if let Ok(h) = merge_hook(m, c, cfg, "https://example.com/r", None, &sink) {
                            assert!(!h.stages.is_empty() || cfg.default_stages.is_empty());
                        }
                    }
                    assert!(manifest.get(&m.id).is_some());
                }
            }
        }
    }
});
