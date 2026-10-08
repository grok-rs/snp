//! Loading, validating and defaulting a configuration file.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{ValidationError, validate_context};
use crate::py;
use crate::schema::Schema;
use crate::validator::Cx;
use crate::value::Value;

/// Error returned by [`load_from_filename`].
///
/// [`Error::source`](std::error::Error::source) is `None` for every
/// variant: the [`Display`](fmt::Display) text already contains everything
/// (the contexts of a validation error, the OS error text of an I/O error),
/// so reporters that print source chains (`anyhow`'s `{:#}`, `eyre`, ...)
/// do not repeat it. The [`io::Error`] of [`LoadError::Io`] is available
/// in its `error` field.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// The file is not a regular file, is not valid UTF-8, could not be
    /// parsed, or does not match the schema. Python cfgv raises (and
    /// re-raises as `exc_tp`) a `ValidationError` with this text.
    ///
    /// When the error is [uncaught](ValidationError::uncaught) (an
    /// exception other than `ValidationError` raised during validation),
    /// Python lets it escape instead: it has no `File` context and is not
    /// re-raised as `exc_tp`.
    Invalid(ValidationError),
    /// Reading the file failed. Python cfgv does not catch this: the
    /// `OSError` raised by `open()` escapes.
    Io {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying error.
        error: io::Error,
    },
}

impl fmt::Display for LoadError {
    /// `Invalid` displays like the [`ValidationError`]. `Io` displays like
    /// CPython's `str(OSError)` on Unix, e.g. `[Errno 13] Permission
    /// denied: 'config.yaml'`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(err) => err.fmt(f),
            Self::Io { path, error } => {
                let path = path.to_string_lossy();
                match error.raw_os_error() {
                    Some(code) => {
                        let text = error.to_string();
                        let suffix = format!(" (os error {code})");
                        let strerror = text.strip_suffix(&suffix).unwrap_or(&text);
                        write!(f, "[Errno {code}] {strerror}: ")?;
                    }
                    None => write!(f, "{error}: ")?,
                }
                py::write_repr_str(f, &path)
            }
        }
    }
}

impl std::error::Error for LoadError {}

impl From<ValidationError> for LoadError {
    fn from(err: ValidationError) -> Self {
        Self::Invalid(err)
    }
}

/// Loads, validates and defaults a configuration file like Python cfgv's
/// `load_from_filename`, discarding warnings.
///
/// See [`load_from_filename_with`].
///
/// # Errors
///
/// See [`load_from_filename_with`].
pub fn load_from_filename<F, E>(
    filename: &Path,
    schema: &Schema,
    load_strategy: F,
    display_filename: Option<&str>,
) -> Result<Value, LoadError>
where
    F: FnOnce(&str) -> Result<Value, E>,
    E: fmt::Display,
{
    load_from_filename_with(
        filename,
        schema,
        load_strategy,
        display_filename,
        &mut Cx::discard(),
    )
}

/// Loads, validates and defaults a configuration file like Python cfgv's
/// `load_from_filename`, emitting validation warnings through `cx`.
///
/// 1. If `filename` is not a regular file (symlinks are followed):
///    `{display_filename} is not a file`, without a `File` context.
/// 2. The file is read and decoded as strict UTF-8, then `\r\n` and lone
///    `\r` are translated to `\n` (Python text mode). A decoding error is
///    reported with CPython's text, e.g. `'utf-8' codec can't decode byte
///    0xff in position 0: invalid start byte`.
/// 3. `load_strategy` parses the text; its error becomes the leaf message
///    (`str(e)` in Python, [`Display`](fmt::Display) here).
/// 4. The value is validated against `schema` and returned with defaults
///    applied.
///
/// Errors from steps 2–4 are wrapped in a `File {display_filename}`
/// context. `display_filename` defaults to `filename` (also when empty, as
/// with Python's `display_filename or filename`).
///
/// Python re-raises every `ValidationError` as `exc_tp(e)`. The text of
/// [`LoadError::Invalid`] is `str(exc_tp(e))` for an `exc_tp` whose `str()`
/// is the wrapped error's, such as pre-commit's `InvalidConfigError`. With
/// Python's default `exc_tp=ValidationError` the text gains a leading
/// `"\n=====> "`; `ValidationError::new(err.to_string())` reproduces that.
///
/// ```no_run
/// use std::path::Path;
/// use cfgv::{Cx, Schema, load_from_filename_with};
///
/// # fn parse_yaml(_: &str) -> Result<cfgv::Value, String> { unimplemented!() }
/// # let schema: Schema = unimplemented!();
/// let mut warnings = Vec::new();
/// let config = load_from_filename_with(
///     Path::new(".pre-commit-config.yaml"),
///     &schema,
///     parse_yaml,
///     None,
///     &mut Cx::collect(&mut warnings),
/// );
/// ```
///
/// # Errors
///
/// Returns [`LoadError::Invalid`] for every case Python reports as a
/// `ValidationError` (and for [uncaught](ValidationError::uncaught)
/// exceptions raised during validation), and [`LoadError::Io`] when the
/// file cannot be read.
pub fn load_from_filename_with<F, E>(
    filename: &Path,
    schema: &Schema,
    load_strategy: F,
    display_filename: Option<&str>,
    cx: &mut Cx<'_>,
) -> Result<Value, LoadError>
where
    F: FnOnce(&str) -> Result<Value, E>,
    E: fmt::Display,
{
    let display = display_filename
        .filter(|s| !s.is_empty())
        .map_or_else(|| filename.display().to_string(), str::to_owned);

    if !filename.is_file() {
        return Err(ValidationError::new(format!("{display} is not a file")).into());
    }
    let bytes = std::fs::read(filename).map_err(|error| LoadError::Io {
        path: filename.to_path_buf(),
        error,
    })?;

    let value = validate_context(
        || format!("File {display}"),
        || {
            let text = py::decode_utf8(&bytes).map_err(|e| ValidationError::new(e.to_string()))?;
            let text = py::universal_newlines(text);
            let data = load_strategy(&text).map_err(|e| ValidationError::new(e.to_string()))?;
            schema.check_with(&data, cx)?;
            Ok(schema.apply_defaults_owned(data))
        },
    )?;
    Ok(value)
}
