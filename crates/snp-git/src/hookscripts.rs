//! 11 §A.5.1 hook-script classifier (pure, bounded file reads: ≤ 1 MiB, multi-pattern scan; lowest class rank wins).
//! Used by snp-cmd (install/uninstall/doctor) and by snp-run's `run` hint (ADR-070).
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-18")]

use std::io;
use std::path::Path;

/// Who wrote a hook script.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Owner {
    /// No file.
    Missing,
    /// A directory, socket, ….
    NotAFile,
    /// snp 2.x (stale: an id from [`SNP_PRIOR_IDS`]).
    Snp {
        /// The script predates the current template.
        stale: bool,
    },
    /// snp 1.x.
    SnpV1,
    /// pre-commit (era of `PRIOR_HASHES`, None for `CURRENT_HASH`).
    PreCommit {
        /// Template era.
        era: Option<u8>,
    },
    /// prek.
    Prek,
    /// lefthook.
    Lefthook,
    /// Anything else.
    Foreign,
}

/// The `# … ID:` marker of the current snp hook-script template.
pub const SNP_SCRIPT_ID: &str = "5bb9bd89cf6c92c58fee7677ceaf4fc3";

/// Ids of earlier snp 2.x templates (a template change bumps the id and moves the old one here). Empty in 2.0.0.
pub const SNP_PRIOR_IDS: &[&str] = &[];

/// Classifies the hook script at `path` (≤ 1 MiB read).
pub fn classify(path: &Path) -> io::Result<Owner> {
    todo!()
}

impl Owner {
    /// A hook manager's script (overwritten in place by `snp install`).
    pub fn is_manager(&self) -> bool {
        todo!()
    }

    /// "pre-commit" | "prek" | "snp 1.x".
    pub fn owner_name(&self) -> Option<&'static str> {
        todo!()
    }
}
