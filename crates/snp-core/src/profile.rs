//! The emulated Python profiles per subsystem and other version constants (ARCHITECTURE §0.3,
//! ADR-020, ADR-050).

/// snp's own version (compared only by `minimum_snp_version`).
pub const SNP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The pre-commit release snp reproduces: compared by `minimum_pre_commit_version`; the compat
/// brand's `--version`.
pub const PRE_COMMIT: &str = "4.6.2";
/// CPython whose argparse texts are reproduced (pre-commit CLI, `pygrep.py`, `hazmat.py`,
/// pre-commit-hooks console scripts).
pub const ARGPARSE_PY: &str = "3.13.14";
/// CPython whose `re` grammar (accept/reject, error texts) is reproduced.
pub const RE_GRAMMAR_PY: &str = "3.12";
/// Unicode data of the regex engines (`regex-syntax` 0.8).
pub const RE_UNICODE: &str = "16.0.0";
/// Unicode data of `repr()`, `str.isprintable` and east-asian width (CPython 3.13).
pub const TEXT_UNICODE: &str = "15.1.0";
/// CPython emulated by the native pre-commit-hooks (json messages, recursion thresholds, …).
pub const PCH_PY: &str = "3.13";
/// identify release whose tag tables are reproduced (`file_identify::IDENTIFY_VERSION`).
pub const IDENTIFY: &str = "2.6.20";
/// Where snp-only texts point (panic hint, crash log, snp brand).
pub const BUG_URL: &str = "https://github.com/grok-rs/snp/issues";
/// Version of the local-hook template bytes, part of local repo keys (snp-store); re-exported
/// by snp-lang.
pub const LOCAL_TEMPLATE_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert_eq!(super::SNP_VERSION, "2.0.0");
        assert_eq!(super::TEXT_UNICODE, snp_pycompat::chars::UNICODE_VERSION);
    }
}
