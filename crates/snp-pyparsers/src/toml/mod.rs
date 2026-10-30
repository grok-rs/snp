//! `check-toml`: CPython 3.13 `tomllib` acceptance and messages (09 §4.C; `check_toml.py`).
//!
//! A statement-for-statement port of `Lib/tomllib/_parser.py` and `_re.py` (TOML 1.0.0; identical in 3.12 and 3.13
//! except 3.13's `MAX_KEY_PARTS`), over the decoded text as code points so positions are Python's. Arbitrary-size
//! integers are accepted up to CPython's 4300-digit `int()` limit; dates are validated like `datetime.date`.
//!
//! `tomllib` is recursive; CPython's recursion limit (1000 Python frames) is reproduced by counting the frames the
//! Python code would have on its stack at every call (the parser itself is iterative), with the `check-toml`
//! console script's own frames below `tomllib.loads`. Past the limit the hook crashes with `RecursionError`
//! (≈ 496 nested arrays, 330 nested inline tables).
//!
//! Owner: WP-17.

mod parse;

use crate::CheckError;
use crate::json::PyMinor;

/// `tomllib.loads(bytes.decode())`; `Err` is the `tomllib.TOMLDecodeError` text.
///
/// Reproduces `check_toml.py` (v4.3.0+) under CPython 3.13: `Err` is the `str()` the hook prints after
/// `{filename}: `, or, for the crash path, the traceback's last line (see [`check_ex`]).
pub fn check(bytes: &[u8]) -> Result<(), String> {
    check_ex(bytes, PyMinor::P313).map_err(CheckError::into_message)
}

/// [`check`] for an emulated CPython minor (ARCHITECTURE §1.4.11 addition), keeping the crash path apart:
/// `UnicodeDecodeError` (the file is not UTF-8), `RecursionError` (nesting beyond the recursion limit; 3.13 also
/// keys of more than 1000 parts) and `ValueError` (a decimal integer over 4300 digits) are crashes.
pub fn check_ex(bytes: &[u8], v: PyMinor) -> Result<(), CheckError> {
    let text = match snp_pycompat::codec::decode_utf8(bytes) {
        Ok(t) => t,
        Err(e) => return Err(CheckError::crash("UnicodeDecodeError", e.to_string())),
    };
    parse::loads(text, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_09() {
        for (input, want) in [
            (
                "a = 1\na = 2\n",
                "Cannot overwrite a value (at line 2, column 6)",
            ),
            ("a = \n", "Invalid value (at line 1, column 5)"),
            ("x = [1,\n2,\n", "Invalid value (at end of document)"),
            ("\u{feff}a = 1", "Invalid statement (at line 1, column 1)"),
            (
                "a = { b = 1, }",
                "Invalid initial character for a key part (at line 1, column 14)",
            ),
            (
                "a = 1 # \u{7}",
                "Found invalid character '\\x07' (at line 1, column 9)",
            ),
            (
                "[a]\n[a]",
                "Cannot declare ('a',) twice (at line 2, column 3)",
            ),
            (
                "a = 1979-05-27T07:32",
                "Expected newline or end of document after a statement (at line 1, column 15)",
            ),
            (
                "a = 12:30",
                "Expected newline or end of document after a statement (at line 1, column 7)",
            ),
            (
                "a = 0x",
                "Expected newline or end of document after a statement (at line 1, column 6)",
            ),
            (
                "a = \"\\e\"",
                "Unescaped '\\' in a string (at line 1, column 8)",
            ),
            (
                "a = 1\rb = 2",
                "Expected newline or end of document after a statement (at line 1, column 6)",
            ),
        ] {
            assert_eq!(check(input.as_bytes()).unwrap_err(), want, "{input:?}");
        }
        for ok in [
            "",
            "a = 1\r\nb = 2\r\n",
            "a = 99999999999999999999",
            "[[t]]\n[[t]]\n",
        ] {
            assert_eq!(check(ok.as_bytes()), Ok(()), "{ok:?}");
        }
        assert!(
            check(b"a = \xff")
                .unwrap_err()
                .starts_with("UnicodeDecodeError: 'utf-8' codec")
        );
    }
}
