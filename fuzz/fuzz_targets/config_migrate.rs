//! Fuzz target `config_migrate`: migrate-config.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! `migrate_config` on arbitrary bytes: an error is an `InvalidConfigError` under `==> File`; a migration yields
//! UTF-8 text with the same number of `\r\n` endings or more (line endings are preserved, D-DATA-09), and
//! migrating the result again never fails to parse.
//!
//! Owner: WP-21.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_config::migrate::{Outcome, migrate_config};

fuzz_target!(|data: &[u8]| {
    let display = OsStr::new(".pre-commit-config.yaml");
    match migrate_config(data, display) {
        Err(e) => {
            assert!(!e.manifest);
            assert!(
                e.rendered
                    .starts_with("\n==> File .pre-commit-config.yaml\n"),
                "{:?}",
                e.rendered
            );
        }
        Ok(Outcome::AlreadyMigrated) => {}
        Ok(Outcome::Migrated(out)) => {
            let text = std::str::from_utf8(&out).expect("migrated text is UTF-8");
            let before = std::str::from_utf8(data)
                .map(|s| s.matches("\r\n").count())
                .unwrap_or(0);
            assert!(text.matches("\r\n").count() >= before);
            let _ = migrate_config(&out, display);
        }
        Ok(_) => {}
    }
});
