//! The converted configs and overlays load with snp-config (05 §19.2 step 5, ARCHITECTURE §8.5): no error, no
//! warning (every emitted extension key is one snp knows), and every local hook finalizes.
//!
//! Owner: WP-34.

#[cfg(test)]
mod validate {
    use std::path::Path;

    use snp_config::{ConfigSource, ExtMode, LoadOpts, RepoConfig};
    use snp_core::brand::Brand;
    use snp_core::output::CollectWarnings;
    use snp_pyre::RegexCache;

    fn goldens() -> Vec<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens");
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_str().unwrap().ends_with(".yaml"))
            .collect();
        v.sort();
        assert!(v.len() >= 10, "{v:?}");
        v
    }

    #[test]
    fn goldens_load_with_snp_config_without_warnings() {
        for golden in goldens() {
            let name = golden.file_name().unwrap().to_str().unwrap().to_owned();
            if name.ends_with(".overlay.yaml") {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".pre-commit-config.yaml");
            std::fs::copy(&golden, &path).unwrap();
            for brand in [Brand::Snp, Brand::PreCommit] {
                let regexes = RegexCache::new();
                let sink = CollectWarnings::new();
                let o = LoadOpts {
                    brand,
                    ext: ExtMode::Enabled,
                    regexes: &regexes,
                    warn: &sink,
                    diag: &sink,
                };
                let src = ConfigSource {
                    path: path.clone(),
                    display: ".pre-commit-config.yaml".into(),
                    project_root: dir.path().to_path_buf(),
                };
                let cfg = snp_config::load_config(&src, &o).unwrap_or_else(|e| panic!("{name}: {}", e.rendered));
                let mut n = 0;
                for repo in &cfg.repos {
                    let RepoConfig::Local { hooks } = repo else {
                        panic!("{name}: only repo: local is emitted");
                    };
                    for h in hooks {
                        snp_config::finalize_local(h, &cfg, "local", None, &sink)
                            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
                        n += 1;
                    }
                }
                assert!(n > 0, "{name}");
                assert_eq!(sink.take(), Vec::<String>::new(), "{name}");
            }
        }
    }

    #[test]
    fn portable_goldens_use_only_pre_commit_keys() {
        for golden in goldens() {
            let name = golden.file_name().unwrap().to_str().unwrap().to_owned();
            if !name.contains(".portable.") {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".pre-commit-config.yaml");
            std::fs::copy(&golden, &path).unwrap();
            let regexes = RegexCache::new();
            let sink = CollectWarnings::new();
            let o = LoadOpts {
                brand: Brand::PreCommit,
                ext: ExtMode::Strict,
                regexes: &regexes,
                warn: &sink,
                diag: &sink,
            };
            let src = ConfigSource {
                path: path.clone(),
                display: ".pre-commit-config.yaml".into(),
                project_root: dir.path().to_path_buf(),
            };
            let cfg = snp_config::load_config(&src, &o).unwrap_or_else(|e| panic!("{name}: {}", e.rendered));
            for repo in &cfg.repos {
                if let RepoConfig::Local { hooks } = repo {
                    for h in hooks {
                        snp_config::finalize_local(h, &cfg, "local", None, &sink).unwrap();
                    }
                }
            }
            assert_eq!(sink.take(), Vec::<String>::new(), "{name}: pre-commit would warn");
        }
    }

    #[test]
    fn overlays_load() {
        let mut seen = 0;
        for golden in goldens() {
            let name = golden.file_name().unwrap().to_str().unwrap().to_owned();
            if !name.ends_with(".overlay.yaml") {
                continue;
            }
            seen += 1;
            let regexes = RegexCache::new();
            let sink = CollectWarnings::new();
            let o = LoadOpts {
                brand: Brand::Snp,
                ext: ExtMode::Enabled,
                regexes: &regexes,
                warn: &sink,
                diag: &sink,
            };
            snp_config::overlay::load(&golden, &o).unwrap_or_else(|e| panic!("{name}: {}", e.rendered));
            assert_eq!(sink.take(), Vec::<String>::new(), "{name}");
        }
        assert!(seen > 0);
    }
}
