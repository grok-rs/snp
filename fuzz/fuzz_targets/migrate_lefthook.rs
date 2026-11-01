//! Fuzz target `migrate_lefthook`: the lefthook converter.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte selects the config format (`.yml`, `.json`, `.jsonc`, `.toml`) and the flags (portable, local,
//! strict); the rest is the config text, split at the first `\n#local\n` into `lefthook.<ext>` and
//! `lefthook-local.yml`. `snp_migrate::lefthook` must either fail with exit 1 (load error) / exit 2 (`--strict`) or
//! produce YAML that snp-yaml loads, an overlay that loads, and a report ending in the summary line. The same bytes
//! also go through `to_builtin` / `to_upstream`, whose output must stay loadable YAML.
//!
//! Owner: WP-34.
#![no_main]

use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use snp_core::failure::Failure;
use snp_migrate::LefthookOpts;

fn workdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("snp-fuzz-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("fuzz work dir");
    d
}

fn loads(text: &str) -> bool {
    snp_yaml::load(text, &snp_yaml::Limits::default()).is_ok()
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let ext = ["yml", "json", "jsonc", "toml"][usize::from(sel & 3)];
    let text = String::from_utf8_lossy(rest);
    let (main, local) = match text.split_once("\n#local\n") {
        Some((m, l)) => (m.to_owned(), Some(l.to_owned())),
        None => (text.into_owned(), None),
    };
    let dir = workdir();
    std::fs::write(dir.join(format!("lefthook.{ext}")), &main).expect("write config");
    if let Some(l) = &local {
        std::fs::write(dir.join("lefthook-local.yml"), l).expect("write local config");
    }
    let o = LefthookOpts {
        root: dir.clone(),
        config: None,
        output: None,
        portable: sel & 4 != 0,
        local: sel & 8 != 0,
        strict: sel & 16 != 0,
    };
    match snp_migrate::lefthook(&o) {
        Ok(c) => {
            assert!(
                loads(&c.config_yaml),
                "unloadable output:\n{}",
                c.config_yaml
            );
            if let Some(ov) = &c.overlay_yaml {
                assert!(loads(ov), "unloadable overlay:\n{ov}");
            }
            assert!(c.report.last().is_some_and(|l| l.starts_with("Converted ")));
            assert!(!(o.strict && c.lossy));
        }
        Err(Failure::SystemExit { code: 1 | 2, .. }) => {}
        Err(other) => panic!("unexpected failure {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    for f in [snp_migrate::to_builtin, snp_migrate::to_upstream] {
        if let Ok(out) = f(rest) {
            let s = std::str::from_utf8(&out).expect("UTF-8 in, UTF-8 out");
            if loads(std::str::from_utf8(rest).unwrap_or("")) {
                assert!(loads(s), "rewrite broke the YAML:\n{s}");
            }
        }
    }
});
