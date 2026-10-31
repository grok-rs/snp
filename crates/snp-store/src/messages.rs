//! Every user-visible text snp-store produces (ARCHITECTURE §1.3 "Messages").
//!
//! The store is snp's own (FD-2): pre-commit has none of these texts, so they read the same in both brands except
//! the lock-contention line, which reproduces `store.py:105-106` (`Locking pre-commit directory`) and names the tool
//! by brand (`brand_map`), and M7, which ARCHITECTURE §5.1 writes with `{prog}`. The ownership refusals (D-SEC-13,
//! ADR-044) are written with a literal `snp` in §5.1 and keep it in both brands.
//!
//! Owner: WP-22.

use std::path::Path;

use snp_core::brand::Brand;

/// The `[INFO]` text logged once per process when a store lock is contended (`store.py:105-106`, ARCHITECTURE
/// §5.5): `Locking pre-commit directory` (compat brand) / `Locking snp directory` (snp brand).
pub fn locking_message(brand: Brand) -> &'static str {
    match brand {
        Brand::PreCommit => "Locking pre-commit directory",
        Brand::Snp => "Locking snp directory",
    }
}

/// M7 (ADR-011, ARCHITECTURE §5.1): `{prog} refuses to use pre-commit's cache directory {path}; set SNP_HOME to
/// another directory`, `{prog}` by brand (`pre-commit` | `snp`).
pub(crate) fn refuse_pre_commit_store(brand: Brand, path: &Path) -> String {
    format!(
        "{} refuses to use pre-commit's cache directory {}; set SNP_HOME to another directory",
        brand.prog(),
        path.display()
    )
}

/// `~` cannot be expanded and neither `SNP_HOME` nor `XDG_CACHE_HOME` is set (11 §A.10.1).
pub(crate) const NO_HOME: &str = "cannot determine the home directory; set SNP_HOME";

/// D-SEC-13 (ADR-077): running as root from a store another (non-root) user owns.
pub(crate) fn refuse_foreign_store_as_root(uid: u32, path: &Path) -> String {
    format!(
        "snp refuses to run as root from a store owned by uid {uid}: {}; set SNP_ALLOW_FOREIGN_STORE=1 to trust it",
        path.display()
    )
}

/// ADR-044: a world-writable store without the sticky bit lets any user plant symlinks in it.
pub(crate) fn refuse_world_writable_store(path: &Path) -> String {
    format!(
        "snp refuses to use the world-writable store {}; remove write access for others or set SNP_HOME to another \
         directory",
        path.display()
    )
}

/// D-DIAG-05 (ADR-044): the one stderr warning for a store owned by another uid.
pub(crate) fn foreign_store_warning(uid: Option<u32>, path: &Path) -> String {
    match uid {
        Some(uid) => format!(
            "[WARNING] the snp store {} is owned by uid {uid}, not by the current user",
            path.display()
        ),
        None => format!(
            "[WARNING] the snp store {} is owned by another user",
            path.display()
        ),
    }
}

/// The first line of the README snp writes into a store it creates (ARCHITECTURE §5.2).
pub(crate) const README: &str = "This directory is maintained by snp (You Shell Not Pass).\nLearn more: https://github.com/grok-rs/snp\n";

/// The first README line of pre-commit's store (`store.py:72-76`): its signature (M7).
pub(crate) const PRE_COMMIT_README_SIGNATURE: &[u8] =
    b"This directory is maintained by the pre-commit project.";

/// The first README line of an snp 1.x store (11 §A.10.3): its leftovers are removed by `sweep`.
pub(crate) const SNP1_README_SIGNATURE: &[u8] =
    b"This directory is maintained by the SNP (Shell Not Pass) project.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden texts in both brands (only the lock line depends on the brand).
    #[test]
    fn goldens() {
        assert_eq!(
            locking_message(Brand::PreCommit),
            "Locking pre-commit directory"
        );
        assert_eq!(locking_message(Brand::Snp), "Locking snp directory");
        assert_eq!(
            refuse_pre_commit_store(Brand::Snp, Path::new("/h/.cache/pre-commit")),
            "snp refuses to use pre-commit's cache directory /h/.cache/pre-commit; set SNP_HOME to another directory"
        );
        assert_eq!(
            refuse_pre_commit_store(Brand::PreCommit, Path::new("/h/.cache/pre-commit")),
            "pre-commit refuses to use pre-commit's cache directory /h/.cache/pre-commit; set SNP_HOME to another \
             directory"
        );
        assert_eq!(
            refuse_foreign_store_as_root(1000, Path::new("/cache/snp")),
            "snp refuses to run as root from a store owned by uid 1000: /cache/snp; set SNP_ALLOW_FOREIGN_STORE=1 to \
             trust it"
        );
        assert_eq!(
            refuse_world_writable_store(Path::new("/tmp/s")),
            "snp refuses to use the world-writable store /tmp/s; remove write access for others or set SNP_HOME to \
             another directory"
        );
        assert_eq!(
            foreign_store_warning(Some(1000), Path::new("/c/snp")),
            "[WARNING] the snp store /c/snp is owned by uid 1000, not by the current user"
        );
        assert_eq!(
            foreign_store_warning(None, Path::new("/c/snp")),
            "[WARNING] the snp store /c/snp is owned by another user"
        );
        assert_eq!(
            README,
            "This directory is maintained by snp (You Shell Not Pass).\nLearn more: https://github.com/grok-rs/snp\n"
        );
        assert_eq!(NO_HOME, "cannot determine the home directory; set SNP_HOME");
    }
}
