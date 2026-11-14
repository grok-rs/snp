//! identify's `extensions` module: the extension and file-name tables.
//!
//! The tables are generated from Python identify
//! ([`IDENTIFY_VERSION`](crate::IDENTIFY_VERSION)) by `cargo xtask
//! gen-identify`; see the crate README.

use crate::tags::TagSet;

pub use crate::generated::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};

/// Tags for an extension key of [`EXTENSIONS`] (exact match, no case
/// folding; empty if unknown).
///
/// [`tags_from_filename`](crate::tags_from_filename) implements identify's
/// full rules (case-insensitive extensions, the binary-check table, names).
#[must_use]
pub fn get_extension_tags(extension: &str) -> TagSet {
    EXTENSIONS.get(extension).unwrap_or_default()
}

/// Tags for an extension key of [`EXTENSIONS_NEED_BINARY_CHECK`] (exact
/// match; empty if unknown). These sets carry no `text` / `binary` tag.
#[must_use]
pub fn get_extensions_need_binary_check_tags(extension: &str) -> TagSet {
    EXTENSIONS_NEED_BINARY_CHECK
        .get(extension)
        .unwrap_or_default()
}

/// Tags for an exact file name of [`NAMES`] (case-sensitive; empty if
/// unknown).
#[must_use]
pub fn get_name_tags(name: &str) -> TagSet {
    NAMES.get(name).unwrap_or_default()
}
