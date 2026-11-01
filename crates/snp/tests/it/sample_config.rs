//! Integration tests: `sample-config` (T10-VAL-04 lives in `validate.rs`).
//!
//! Owner: WP-33.

use crate::cli::{assert_code, snp, tenv, text};

/// Both brands print the same sample; `PRE_COMMIT_COLOR=bogus` is a usage error only without `--color` (SM-09).
#[test]
fn sample_config_both_brands_and_color_env() {
    let env = tenv();
    let a = snp(&env, env.root(), &["sample-config"]);
    assert_code(&a, 0);
    assert!(text(&a.stdout).starts_with("# See https://pre-commit.com for more information\n"));
    let env = env.with_env("PRE_COMMIT_COLOR", "bogus");
    let ok = snp(&env, env.root(), &["sample-config", "--color", "never"]);
    assert_code(&ok, 0);
    let bad = snp(&env, env.root(), &["sample-config"]);
    assert_code(&bad, 2);
    assert!(text(&bad.stderr).ends_with(
        "snp sample-config: error: argument --color: invalid use_color value: 'bogus'\n"
    ));
}
