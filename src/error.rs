//! Validation errors and their context chain.

use std::fmt;

/// A validation failure: a leaf message plus the chain of contexts (outermost
/// first) in which it occurred.
///
/// [`Display`](fmt::Display) renders exactly like Python cfgv's
/// `str(ValidationError)`: a leading newline, one `==> {context}` line per
/// context, then `=====> {message}`, with no trailing newline.
///
/// ```
/// use cfgv::ValidationError;
///
/// let err = ValidationError::new("Missing required key: id")
///     .with_context("At Hook(id=MISSING)")
///     .with_context("File .pre-commit-hooks.yaml");
/// assert_eq!(
///     err.to_string(),
///     "\n==> File .pre-commit-hooks.yaml\n==> At Hook(id=MISSING)\n=====> Missing required key: id",
/// );
/// assert_eq!(err.message(), "Missing required key: id");
/// ```
///
/// [`Error::source`](std::error::Error::source) is always `None`: the
/// contexts are part of this error's own text, so error reporters that walk
/// source chains do not print them twice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationError {
    message: String,
    /// Innermost context first (contexts are added while unwinding).
    contexts: Vec<String>,
}

impl ValidationError {
    /// Creates an error with a leaf message and no context.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            contexts: Vec::new(),
        }
    }

    /// The leaf message (the text after `=====> `).
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The contexts, outermost first (the texts after `==> `).
    pub fn contexts(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.contexts.iter().rev().map(String::as_str)
    }

    /// Wraps the error in one more (outer) context, like Python's
    /// `validate_context(ctx)`.
    #[must_use]
    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.contexts.push(ctx.into());
        self
    }

    /// All contexts (outermost first) followed by the leaf message.
    #[must_use]
    pub fn trace_parts(&self) -> Vec<&str> {
        self.contexts()
            .chain(std::iter::once(self.message.as_str()))
            .collect()
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\n")?;
        for ctx in self.contexts() {
            writeln!(f, "==> {ctx}")?;
        }
        write!(f, "=====> {}", self.message)
    }
}

impl std::error::Error for ValidationError {}

/// Runs `f` and wraps any [`ValidationError`] it returns in the context
/// produced by `ctx`, like Python's `with validate_context(ctx):`.
///
/// The context is built lazily, so the success path does not allocate.
///
/// # Errors
///
/// Returns the error of `f`, wrapped in the context.
///
/// ```
/// use cfgv::{ValidationError, validate_context};
///
/// let err = validate_context(|| "At key: id".to_owned(), || {
///     Err::<(), _>(ValidationError::new("Expected string got int"))
/// })
/// .unwrap_err();
/// assert_eq!(err.trace_parts(), ["At key: id", "Expected string got int"]);
/// ```
pub fn validate_context<T, F, C>(ctx: C, f: F) -> Result<T, ValidationError>
where
    C: FnOnce() -> String,
    F: FnOnce() -> Result<T, ValidationError>,
{
    f().map_err(|e| e.with_context(ctx()))
}
