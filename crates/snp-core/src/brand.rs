//! The brand: compat (`pre-commit` name, byte-identical texts) or snp (ADR-023, 11 §A.8.1).

use std::ffi::OsStr;
use std::path::Path;

use crate::profile;

/// Which product name user-facing text uses (FD-1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Brand {
    /// Invoked as `pre-commit`: every byte as pre-commit 4.6.2.
    PreCommit,
    /// Any other name: same layout, product name `snp`.
    Snp,
}

impl Brand {
    /// `basename(argv[0])` minus a trailing `.exe` (ASCII-case-insensitive, Windows only) equal to
    /// `pre-commit` (exact on Unix, ASCII-case-insensitive on Windows) ⇒ [`Brand::PreCommit`].
    /// Never `current_exe()` (Linux resolves symlinks, hiding the alias). No env override.
    pub fn detect(arg0: Option<&OsStr>) -> Self {
        let Some(name) = arg0.and_then(|a| Path::new(a).file_name()) else {
            return Self::Snp;
        };
        let name = name.as_encoded_bytes();
        let is_compat = if cfg!(windows) {
            let stem = match name
                .len()
                .checked_sub(4)
                .and_then(|i| name.split_at_checked(i))
            {
                Some((stem, ext)) if ext.eq_ignore_ascii_case(b".exe") => stem,
                _ => name,
            };
            stem.eq_ignore_ascii_case(b"pre-commit")
        } else {
            name == b"pre-commit"
        };
        if is_compat {
            Self::PreCommit
        } else {
            Self::Snp
        }
    }

    /// The argparse `prog` and the tool's name: `pre-commit` | `snp`.
    pub const fn prog(self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit",
            Self::Snp => "snp",
        }
    }

    /// `--version` output (without newline): `pre-commit 4.6.2` | `snp 2.0.0` (ADR-023).
    pub fn version_line(self) -> String {
        match self {
            Self::PreCommit => format!("pre-commit {}", profile::PRE_COMMIT),
            Self::Snp => format!("snp {}", profile::SNP_VERSION),
        }
    }

    /// The crash log's file name in the store (ADR-013).
    pub const fn crash_log_name(self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit.log",
            Self::Snp => "snp.log",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(s: &str) -> Brand {
        Brand::detect(Some(OsStr::new(s)))
    }

    #[test]
    fn detect_table() {
        assert_eq!(detect("pre-commit"), Brand::PreCommit);
        assert_eq!(detect("/x/pre-commit"), Brand::PreCommit);
        assert_eq!(detect("./bin/pre-commit"), Brand::PreCommit);
        assert_eq!(detect("snp"), Brand::Snp);
        assert_eq!(detect("/usr/bin/snp"), Brand::Snp);
        assert_eq!(detect("pre-commit-x"), Brand::Snp);
        assert_eq!(detect("xpre-commit"), Brand::Snp);
        assert_eq!(detect(""), Brand::Snp);
        assert_eq!(Brand::detect(None), Brand::Snp);
        let windows_only = if cfg!(windows) {
            Brand::PreCommit
        } else {
            Brand::Snp
        };
        assert_eq!(detect("pre-commit.exe"), windows_only);
        assert_eq!(detect("PRE-COMMIT.EXE"), windows_only);
        assert_eq!(detect("Pre-Commit"), windows_only);
        assert_eq!(detect("pre-commit.exe.exe"), Brand::Snp);
    }

    #[test]
    fn names() {
        assert_eq!(Brand::PreCommit.prog(), "pre-commit");
        assert_eq!(Brand::Snp.prog(), "snp");
        assert_eq!(Brand::PreCommit.version_line(), "pre-commit 4.6.2");
        assert_eq!(Brand::Snp.version_line(), "snp 2.0.0");
        assert_eq!(Brand::PreCommit.crash_log_name(), "pre-commit.log");
        assert_eq!(Brand::Snp.crash_log_name(), "snp.log");
    }
}
