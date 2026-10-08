//! `sample-config` (`commands/sample_config.py`; 01 §11).
//!
//! Owner: WP-21.

/// `SAMPLE_CONFIG` byte for byte (10 lines, LF, final newline; sha256 `8d958ac8…`, 01 §11).
pub const SAMPLE_CONFIG: &str = "\
# See https://pre-commit.com for more information
# See https://pre-commit.com/hooks.html for more hooks
repos:
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v3.2.0
    hooks:
    -   id: trailing-whitespace
    -   id: end-of-file-fixer
    -   id: check-yaml
    -   id: check-added-large-files
";
