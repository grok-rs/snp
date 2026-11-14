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
///
/// # Uncaught exceptions
///
/// Python validation code can raise exceptions other than
/// `ValidationError`: `re.compile` raises `OverflowError` for
/// `a{4294967296}`, `repr()` of an integer with more than 4300 digits
/// raises `ValueError`, and pre-commit's own check functions raise
/// `ValueError` from `int()`. cfgv's `validate_context` does not catch them,
/// so they escape validation without any context, and `load_from_filename`
/// does not wrap them in `exc_tp`; pre-commit then reports a crash.
///
/// [`ValidationError::uncaught`] represents such an exception so that it
/// can travel through the same `Result`s. An uncaught error ignores
/// [`with_context`](ValidationError::with_context), reports its Python type
/// through [`uncaught_type`](ValidationError::uncaught_type), and displays
/// like the last line of a Python traceback (`{type}: {message}`).
///
/// ```
/// use cfgv::{ValidationError, validate_context};
///
/// let err = validate_context(|| "At key: id".to_owned(), || {
///     Err::<(), _>(ValidationError::uncaught("OverflowError", "the repetition number is too large"))
/// })
/// .unwrap_err();
/// assert_eq!(err.uncaught_type(), Some("OverflowError"));
/// assert_eq!(err.contexts().len(), 0);
/// assert_eq!(err.to_string(), "OverflowError: the repetition number is too large");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationError {
    message: String,
    /// Innermost context first (contexts are added while unwinding).
    contexts: Vec<String>,
    /// The Python exception type of an uncaught exception.
    uncaught: Option<Box<str>>,
}

impl ValidationError {
    /// Creates an error with a leaf message and no context.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            contexts: Vec::new(),
            uncaught: None,
        }
    }

    /// Creates an error standing for a Python exception of type `exc_type`
    /// (e.g. `"ValueError"`) other than `ValidationError`, which Python cfgv
    /// lets escape. See [Uncaught exceptions](#uncaught-exceptions).
    ///
    /// `message` is Python's `str(e)`.
    #[must_use]
    pub fn uncaught(exc_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            contexts: Vec::new(),
            uncaught: Some(exc_type.into().into_boxed_str()),
        }
    }

    /// The Python exception type of an [uncaught](ValidationError::uncaught)
    /// exception; `None` for a validation error.
    #[must_use]
    pub fn uncaught_type(&self) -> Option<&str> {
        self.uncaught.as_deref()
    }

    /// Whether this error stands for an exception Python cfgv does not
    /// catch (see [`ValidationError::uncaught`]).
    #[must_use]
    pub const fn is_uncaught(&self) -> bool {
        self.uncaught.is_some()
    }

    /// The leaf message (the text after `=====> `), or `str(e)` of an
    /// uncaught exception.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The contexts, outermost first (the texts after `==> `).
    pub fn contexts(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.contexts.iter().rev().map(String::as_str)
    }

    /// Wraps the error in one more (outer) context, like Python's
    /// `validate_context(ctx)`. An [uncaught](ValidationError::uncaught)
    /// error is returned unchanged, because `validate_context` only catches
    /// `ValidationError`.
    #[must_use]
    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        if self.uncaught.is_none() {
            self.contexts.push(ctx.into());
        }
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
    /// Python's `str(ValidationError)`; for an uncaught exception, the last
    /// line of its traceback: `{type}: {message}` (`{type}` alone when the
    /// message is empty).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(exc_type) = &self.uncaught {
            f.write_str(exc_type)?;
            if !self.message.is_empty() {
                write!(f, ": {}", self.message)?;
            }
            return Ok(());
        }
        f.write_str("\n")?;
        for ctx in self.contexts() {
            writeln!(f, "==> {ctx}")?;
        }
        write!(f, "=====> {}", self.message)
    }
}

impl std::error::Error for ValidationError {}

impl From<crate::value::MaxStrDigitsError> for ValidationError {
    /// The uncaught `ValueError` that CPython raises when formatting an
    /// integer with more than 4300 digits.
    fn from(err: crate::value::MaxStrDigitsError) -> Self {
        Self::uncaught("ValueError", err.to_string())
    }
}

/// Runs `f` and wraps any [`ValidationError`] it returns in the context
/// produced by `ctx`, like Python's `with validate_context(ctx):`.
/// [Uncaught](ValidationError::uncaught) errors pass through unchanged.
///
/// The context is built lazily, so the success path does not allocate.
/// Python formats it eagerly; when formatting it could raise (`repr()` of an
/// integer beyond CPython's digit limit), check that first.
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
