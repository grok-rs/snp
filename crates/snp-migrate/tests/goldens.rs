//! Golden conversions: lefthook fixtures → committed `.pre-commit-config.yaml` / overlay / report (05 §19.11,
//! `tests/integration/{job_merging,many_extends_levels}.txt` of lefthook). `cargo test -p snp-migrate --test goldens
//! -- --ignored regenerate_goldens` rewrites the goldens after an intended change.
//!
//! Owner: WP-34.

#[cfg(test)]
mod goldens {
    use std::path::{Path, PathBuf};

    use snp_migrate::{Converted, LefthookOpts};

    /// `(fixture, portable, local)`.
    pub(crate) const CASES: &[(&str, bool, bool)] = &[
        ("worked_example", false, false),
        ("worked_example", true, false),
        ("job_merging", false, false),
        ("job_merging", true, false),
        ("many_extends_levels", false, false),
        ("many_extends_levels", true, false),
        ("features", false, false),
        ("features", true, false),
        ("features", false, true),
        ("runnable", false, false),
        ("runnable", true, false),
    ];

    pub(crate) fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    pub(crate) fn golden_base(name: &str, portable: bool, local: bool) -> PathBuf {
        let mode = if portable { "portable" } else { "snp" };
        let local = if local { ".local" } else { "" };
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens")
            .join(format!("{name}.{mode}{local}"))
    }

    pub(crate) fn convert(name: &str, portable: bool, local: bool) -> Converted {
        snp_migrate::lefthook(&LefthookOpts {
            root: fixture(name),
            config: None,
            output: None,
            portable,
            local,
            strict: false,
        })
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    fn files(c: &Converted) -> Vec<(&'static str, String)> {
        let mut v = vec![("yaml", c.config_yaml.clone()), ("report.txt", c.report.join("\n") + "\n")];
        if let Some(o) = &c.overlay_yaml {
            v.push(("overlay.yaml", o.clone()));
        }
        v
    }

    #[test]
    fn conversions_match_goldens() {
        for &(name, portable, local) in CASES {
            let c = convert(name, portable, local);
            let base = golden_base(name, portable, local);
            for (ext, text) in files(&c) {
                let p = base.with_extension(format!("{}.{ext}", base.extension().unwrap().to_str().unwrap()));
                let want = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                assert_eq!(text, want, "{}", p.display());
            }
        }
    }

    #[test]
    #[ignore = "writes goldens"]
    fn regenerate_goldens() {
        std::fs::create_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")).unwrap();
        for &(name, portable, local) in CASES {
            let c = convert(name, portable, local);
            let base = golden_base(name, portable, local);
            for (ext, text) in files(&c) {
                let p = base.with_extension(format!("{}.{ext}", base.extension().unwrap().to_str().unwrap()));
                std::fs::write(&p, text).unwrap();
            }
        }
    }
}
