//! [`CheckError`]: a reported error versus a Python crash (09 §2.6).
//!
//! Owner: WP-17.

use std::fmt;

use snp_core::failure::PyExc;

/// How a check failed, as the hook process experiences it (ARCHITECTURE §1.4.11 addition, see the WP-17 report).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CheckError {
    /// The exception the hook catches (`ValueError`/`JSONDecodeError`, `YAMLError`, `TOMLDecodeError`,
    /// `SAXException`): its `str()`, which the hook prints before going on with the next file.
    Reported(String),
    /// An exception the hook does not catch: Python prints a traceback ending in `{ty}: {text}` and exits 1 without
    /// processing the rest of the partition (09 §2.6). `ty` is the class as the traceback's last line names it
    /// (`RecursionError`, `ruamel.yaml.constructor.ConstructorError`, …).
    Crash(PyExc),
}

impl CheckError {
    /// A crash with class `ty` and text `text`.
    pub(crate) fn crash(ty: &'static str, text: impl Into<String>) -> Self {
        Self::Crash(PyExc::new(ty, text.into().into_bytes()))
    }

    /// The text of the frozen `Result<_, String>` API: the reported message, or the crash's traceback line
    /// `Type: text`.
    pub fn into_message(self) -> String {
        match self {
            Self::Reported(msg) => msg,
            Self::Crash(exc) => crash_line(&exc),
        }
    }

    /// Whether this is the crash path.
    pub const fn is_crash(&self) -> bool {
        matches!(self, Self::Crash(_))
    }
}

/// `Type: text` (or `Type` alone for an empty text), as a traceback's last line shows an exception.
fn crash_line(exc: &PyExc) -> String {
    let text = String::from_utf8_lossy(&exc.text);
    if text.is_empty() {
        exc.ty.to_string()
    } else {
        format!("{}: {text}", exc.ty)
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reported(msg) => f.write_str(msg),
            Self::Crash(exc) => f.write_str(&crash_line(exc)),
        }
    }
}

impl std::error::Error for CheckError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        assert_eq!(CheckError::Reported("x".into()).into_message(), "x");
        let c = CheckError::crash("RecursionError", "maximum recursion depth exceeded");
        assert!(c.is_crash());
        assert_eq!(
            c.to_string(),
            "RecursionError: maximum recursion depth exceeded"
        );
        assert_eq!(CheckError::crash("KeyError", "").into_message(), "KeyError");
    }
}
