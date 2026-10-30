//! Fuzz target `config_load_validate`: config load + validation.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is written to a per-process file and loaded with `load_config` in both brands and both extension
//! modes; a valid config is merged hook by hook (`finalize_local` for local/meta hooks, `merge_hook` against a fixed
//! manifest for remote ones), dumped as YAML and JSON, and loaded once more as a local overlay. Invariants beyond
//! "no panic": a validation error renders with a leading newline and its `==> File` context; an uncaught error
//! (exit 3) is an OS error, never a validation failure.
//!
//! Owner: WP-21.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use snp_config::{
    ConfigSource, ExtMode, LoadOpts, RepoConfig, finalize_local, merge_hook, overlay,
};
use snp_core::brand::Brand;
use snp_core::output::CollectWarnings;
use snp_pyre::RegexCache;

const MANIFEST: &[u8] = b"- {id: h, name: h, entry: echo, language: system}\n\
- {id: trailing-whitespace, name: t, entry: t, language: python, types: [text]}\n";

fn dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("snp-fuzz-config-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    })
}

fuzz_target!(|data: &[u8]| {
    let path = dir().join("cfg.yaml");
    if std::fs::write(&path, data).is_err() {
        return;
    }
    let src = ConfigSource {
        path: path.clone(),
        display: "cfg.yaml".into(),
        project_root: dir().clone(),
    };
    for brand in [Brand::PreCommit, Brand::Snp] {
        for ext in [ExtMode::Enabled, ExtMode::Strict] {
            let regexes = RegexCache::new();
            let sink = CollectWarnings::new();
            let o = LoadOpts {
                brand,
                ext,
                regexes: &regexes,
                warn: &sink,
                diag: &sink,
            };
            match snp_config::load_config(&src, &o) {
                Err(e) => {
                    if e.uncaught().is_none() {
                        assert!(
                            e.rendered.starts_with("\n==> File cfg.yaml\n"),
                            "{:?}",
                            e.rendered
                        );
                    } else {
                        assert!(e.rendered.contains("Error: "), "{:?}", e.rendered);
                    }
                }
                Ok(cfg) => {
                    let manifest =
                        snp_config::load_manifest_bytes(MANIFEST, std::ffi::OsStr::new("m"), &o)
                            .ok();
                    for repo in &cfg.repos {
                        match repo {
                            RepoConfig::Local { hooks } | RepoConfig::Meta { hooks } => {
                                for h in hooks {
                                    let _ = finalize_local(h, &cfg, "local", None, &sink);
                                }
                            }
                            RepoConfig::Remote { hooks, .. } | RepoConfig::Builtin { hooks } => {
                                for h in hooks {
                                    if let Some(m) = manifest.as_ref().and_then(|m| m.get(&h.id)) {
                                        let _ = merge_hook(m, h, &cfg, "r", None, &sink);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    let _ = snp_config::effective_dump(&cfg, false);
                    let _ = snp_config::effective_dump(&cfg, true);
                }
            }
            let _ = overlay::load(&path, &o);
        }
    }
});
