//! `MarkedYAMLError` / `ReaderError` texts of PyYAML's C loader.
//!
//! Owner: WP-14.

use std::borrow::Cow;
use std::fmt;

use crate::messages::STREAM_NAME;

/// A load error; Display == `str(exc)` of the exception PyYAML 6.0.3 raises from `yaml.load(text, CSafeLoader)`
/// (`yaml/error.py:37-75`, `_yaml.pyx` `CParser._parser_error`), which is what cfgv reports as the leaf message:
///
/// * scanner, parser, composer and constructor errors render as `MarkedYAMLError.__str__`: the context, the context
///   mark (only when it differs from the problem mark), the problem, the problem mark and the note, joined by `\n`;
///   a mark is `  in "<unicode string>", line L, column C` (1-based, no snippet: the C loader has none);
/// * reader errors render as `unacceptable character #x%04x: <reason>\n  in "<unicode string>", position N`
///   (`N` is the byte offset, as in libyaml);
/// * the Python exceptions that escape `SafeConstructor` (`ValueError`, `KeyError`, `IndexError`, …) render as
///   `str(exc)` alone;
/// * snp's resource limits (D-SEC-11) render like a marked error with snp's own problem text.
#[derive(Debug, Clone)]
pub struct YamlError {
    repr: Box<Repr>,
}

#[derive(Debug, Clone)]
enum Repr {
    /// `MarkedYAMLError` (`ScannerError`, `ParserError`, `ComposerError`, `ConstructorError`) or a limit error.
    Marked {
        class: &'static str,
        context: Option<Cow<'static, str>>,
        context_mark: Option<Mark>,
        problem: Cow<'static, str>,
        problem_mark: Option<Mark>,
    },
    /// `ReaderError` with an `int` character.
    Reader {
        character: u32,
        position: usize,
        reason: Cow<'static, str>,
    },
    /// Any other Python exception: its class and `str(exc)`.
    Python { class: &'static str, text: String },
}

impl YamlError {
    /// A `MarkedYAMLError` of class `class`.
    pub(crate) fn marked(
        class: &'static str,
        context: Option<Cow<'static, str>>,
        context_mark: Option<Mark>,
        problem: impl Into<Cow<'static, str>>,
        problem_mark: Option<Mark>,
    ) -> Self {
        Self {
            repr: Box::new(Repr::Marked {
                class,
                context,
                context_mark,
                problem: problem.into(),
                problem_mark,
            }),
        }
    }

    /// A `ComposerError` (`_yaml.pyx`).
    pub(crate) fn composer(
        context: Option<&'static str>,
        context_mark: Option<Mark>,
        problem: &'static str,
        problem_mark: Mark,
    ) -> Self {
        Self::marked(
            "ComposerError",
            context.map(Cow::Borrowed),
            context_mark,
            problem,
            Some(problem_mark),
        )
    }

    /// A `ConstructorError(None, None, problem, mark)`.
    pub(crate) fn constructor(problem: impl Into<Cow<'static, str>>, mark: Mark) -> Self {
        Self::marked("ConstructorError", None, None, problem, Some(mark))
    }

    /// A `ConstructorError(context, context_mark, problem, problem_mark)`.
    pub(crate) fn constructor_in(
        context: &'static str,
        context_mark: Mark,
        problem: impl Into<Cow<'static, str>>,
        problem_mark: Mark,
    ) -> Self {
        Self::marked(
            "ConstructorError",
            Some(Cow::Borrowed(context)),
            Some(context_mark),
            problem,
            Some(problem_mark),
        )
    }

    /// A `ReaderError` for an unacceptable character.
    pub(crate) fn reader(character: u32, position: usize, reason: &'static str) -> Self {
        Self {
            repr: Box::new(Repr::Reader {
                character,
                position,
                reason: Cow::Borrowed(reason),
            }),
        }
    }

    /// A Python exception other than `YAMLError` (`ValueError`, `KeyError`, …) with its `str()`.
    pub(crate) fn python(class: &'static str, text: impl Into<String>) -> Self {
        Self {
            repr: Box::new(Repr::Python {
                class,
                text: text.into(),
            }),
        }
    }

    /// One of snp's resource limits (D-SEC-11), reported like a marked error.
    pub(crate) fn limit(problem: String, mark: Option<Mark>) -> Self {
        Self::marked("YAMLError", None, None, problem, mark)
    }

    /// The Python class of the exception PyYAML raises for the same input (`ScannerError`, `ParserError`,
    /// `ComposerError`, `ConstructorError`, `ReaderError`, `ValueError`, `KeyError`, `IndexError`,
    /// `AttributeError`, `TypeError`, `OverflowError`), for crash lines such as
    /// `An unexpected error has occurred: ScannerError: …`. snp's own limit errors (D-SEC-11) report the
    /// base class `YAMLError`.
    #[must_use]
    pub fn py_type(&self) -> &'static str {
        match &*self.repr {
            Repr::Marked { class, .. } | Repr::Python { class, .. } => class,
            Repr::Reader { .. } => "ReaderError",
        }
    }
}

/// `Mark.__str__` of the C loader: no snippet.
fn write_mark(f: &mut fmt::Formatter<'_>, m: &Mark) -> fmt::Result {
    write!(
        f,
        "  in \"{STREAM_NAME}\", line {}, column {}",
        m.line.saturating_add(1),
        m.column.saturating_add(1)
    )
}

impl fmt::Display for YamlError {
    /// `str(exc)` (`MarkedYAMLError.__str__`, `ReaderError.__str__`, or the Python exception's text).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.repr {
            Repr::Marked {
                context,
                context_mark,
                problem,
                problem_mark,
                ..
            } => {
                let mut first = true;
                let mut sep = |f: &mut fmt::Formatter<'_>| {
                    if first {
                        first = false;
                        Ok(())
                    } else {
                        f.write_str("\n")
                    }
                };
                if let Some(context) = context {
                    sep(f)?;
                    f.write_str(context)?;
                }
                if let Some(cm) = context_mark {
                    let differs = problem_mark
                        .as_ref()
                        .is_none_or(|pm| pm.line != cm.line || pm.column != cm.column);
                    if differs {
                        sep(f)?;
                        write_mark(f, cm)?;
                    }
                }
                sep(f)?;
                f.write_str(problem)?;
                if let Some(pm) = problem_mark {
                    sep(f)?;
                    write_mark(f, pm)?;
                }
                Ok(())
            }
            Repr::Reader {
                character,
                position,
                reason,
            } => write!(
                f,
                "unacceptable character #x{character:04x}: {reason}\n  in \"{STREAM_NAME}\", position {position}"
            ),
            Repr::Python { text, .. } => f.write_str(text),
        }
    }
}

impl std::error::Error for YamlError {}

/// A position in the text (PyYAML `Mark`; 0-based line and column).
///
/// As in PyYAML's C loader (libyaml 0.2.5), `index` counts characters (code points) from the start of the stream
/// after a leading byte-order mark, and `column` counts characters from the start of the line. [`crate::Node::span`]
/// holds the byte offsets for slicing the text.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Character index.
    pub index: usize,
    /// 0-based line.
    pub line: usize,
    /// 0-based column.
    pub column: usize,
}
