//! identify's `interpreters` module: the shebang interpreter table.

use crate::tags::TagSet;

pub use crate::generated::INTERPRETERS;

/// Tags for an exact interpreter name of [`INTERPRETERS`] (empty if
/// unknown).
///
/// [`tags_from_interpreter`](crate::tags_from_interpreter) implements
/// identify's full rules (path stripping and version fallback such as
/// `python3.12` → `python3`).
#[must_use]
pub fn get_interpreter_tags(interpreter: &str) -> TagSet {
    INTERPRETERS.get(interpreter).unwrap_or_default()
}
