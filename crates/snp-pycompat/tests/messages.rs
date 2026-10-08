//! Goldens of every user-visible text of the crate (ARCHITECTURE §1.3 "Messages"; the definition of
//! done asks for "messages golden-tested in both brands"): CPython's exception texts, which name no
//! tool and are therefore the same bytes under the compat and the snp brand. The vector files prove
//! them against CPython; these goldens pin the exact bytes and the `py_type()` crash-line class of
//! every variant.
//!
//! Owner: WP-11.

#[cfg(test)]
mod messages {
    use std::ffi::OsStr;

    use snp_pycompat::PathError;
    use snp_pycompat::codec::{self, Utf8DecodeError, Utf8EncodeError, Utf8Reason};
    use snp_pycompat::int::{self, IntParseError};
    use snp_pycompat::repr;
    use snp_pycompat::shlex::ShlexError;
    use snp_pycompat::text::UnpackError;

    #[test]
    fn path_errors() {
        let cases = [
            (PathError::NoPathSpecified, "no path specified"),
            (
                PathError::EmptySequence,
                "commonpath() arg is an empty sequence",
            ),
            (
                PathError::MixAbsRel,
                "Can't mix absolute and relative paths",
            ),
            (
                PathError::DifferentDrives,
                "Paths don't have the same drive",
            ),
            (
                PathError::DifferentMounts {
                    path_drive: "D:".into(),
                    start_drive: "C:".into(),
                },
                "path is on mount 'D:', start on mount 'C:'",
            ),
            (
                PathError::DifferentMounts {
                    path_drive: "\\\\srv\\share".into(),
                    start_drive: "C:".into(),
                },
                "path is on mount '\\\\\\\\srv\\\\share', start on mount 'C:'",
            ),
            (
                PathError::DifferentMounts {
                    path_drive: String::new(),
                    start_drive: "it's:".into(),
                },
                "path is on mount '', start on mount \"it's:\"",
            ),
        ];
        for (e, want) in cases {
            assert_eq!(e.to_string(), want);
            assert_eq!(e.py_type(), "ValueError");
        }
    }

    #[test]
    fn unpack_errors() {
        let e = UnpackError {
            expected: 4,
            got: 3,
        };
        assert_eq!(
            e.to_string(),
            "not enough values to unpack (expected 4, got 3)"
        );
        assert_eq!(e.py_type(), "ValueError");
        assert_eq!(
            UnpackError {
                expected: 4,
                got: 0
            }
            .to_string(),
            "not enough values to unpack (expected 4, got 0)"
        );
        assert_eq!(
            UnpackError {
                expected: 4,
                got: 5
            }
            .to_string(),
            "too many values to unpack (expected 4)"
        );
    }

    #[test]
    fn codec_errors() {
        let byte = |start, first_byte, reason| Utf8DecodeError {
            start,
            end: start + 1,
            first_byte,
            reason,
        };
        let cases = [
            (
                byte(14, 0xe9, Utf8Reason::InvalidContinuationByte),
                "'utf-8' codec can't decode byte 0xe9 in position 14: invalid continuation byte",
            ),
            (
                byte(0, 0xff, Utf8Reason::InvalidStartByte),
                "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
            ),
            (
                byte(1, 0xc3, Utf8Reason::UnexpectedEndOfData),
                "'utf-8' codec can't decode byte 0xc3 in position 1: unexpected end of data",
            ),
            (
                Utf8DecodeError {
                    start: 3,
                    end: 5,
                    first_byte: 0xe2,
                    reason: Utf8Reason::UnexpectedEndOfData,
                },
                "'utf-8' codec can't decode bytes in position 3-4: unexpected end of data",
            ),
        ];
        for (e, want) in cases {
            assert_eq!(e.to_string(), want);
            assert_eq!(e.py_type(), "UnicodeDecodeError");
        }
        // the X13 crash text of `run --all-files` on a committed `caf\xe9.txt` (15 §A2.4, OBS)
        let ls = b".pre-commit-config.yaml\0caf\xe9.txt\0";
        let e = codec::decode_utf8(ls).unwrap_err();
        assert_eq!(
            format!("{}: {e}", e.py_type()),
            "UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 27: invalid continuation byte"
        );
        let e = Utf8EncodeError {
            position: 5,
            surrogate: 0xdcff,
        };
        assert_eq!(
            e.to_string(),
            "'utf-8' codec can't encode character '\\udcff' in position 5: surrogates not allowed"
        );
        assert_eq!(e.py_type(), "UnicodeEncodeError");
    }

    #[test]
    fn int_and_shlex_errors() {
        let e = IntParseError {
            literal: "x".into(),
        };
        assert_eq!(e.to_string(), "invalid literal for int() with base 10: 'x'");
        assert_eq!(e.py_type(), "ValueError");
        // `minimum_pre_commit_version: "1.x"` (X13-CFG-01, D-CRASH-01)
        assert_eq!(
            int::parse_version("1.x").unwrap_err().to_string(),
            "invalid literal for int() with base 10: 'x'"
        );
        assert_eq!(
            ShlexError::NoClosingQuotation.to_string(),
            "No closing quotation"
        );
        assert_eq!(
            ShlexError::NoEscapedCharacter.to_string(),
            "No escaped character"
        );
        assert_eq!(ShlexError::NoClosingQuotation.py_type(), "ValueError");
        assert_eq!(ShlexError::NoEscapedCharacter.py_type(), "ValueError");
    }

    #[test]
    fn repr_built_texts() {
        assert_eq!(repr::fstring_eq("prefix", "sub/"), "prefix='sub/'");
        assert_eq!(
            repr::oserror_str(
                2,
                "No such file or directory",
                Some(OsStr::new("./x.sh")),
                None
            ),
            "[Errno 2] No such file or directory: './x.sh'"
        );
        assert_eq!(
            repr::stdlib_called_process_error_str(
                128,
                &repr::repr_str_tuple(&["git", "rev-list", "refs/heads/a"])
            ),
            "Command '('git', 'rev-list', 'refs/heads/a')' returned non-zero exit status 128."
        );
        assert_eq!(
            repr::stdlib_called_process_error_str(-99, "('git',)"),
            "Command '('git',)' died with unknown signal 99."
        );
        #[cfg(unix)]
        assert_eq!(
            repr::stdlib_called_process_error_str(-15, "('git', 'x')"),
            "Command '('git', 'x')' died with <Signals.SIGTERM: 15>."
        );
        assert_eq!(
            repr::int_literal_error("1.2-dev"),
            "invalid literal for int() with base 10: '1.2-dev'"
        );
        assert_eq!(
            repr::repr_str_list(&["origin"]),
            "['origin']",
            "hook-impl for pre-push expected 2 arguments but got 1: ['origin']"
        );
    }
}
