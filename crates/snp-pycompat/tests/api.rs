//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.2 with
//! explicit types, so signature drift breaks compilation, plus the trait-bound assertions of the
//! named iterator and error types.
//!
//! Owner: WP-11 (created by WP-00).

#[cfg(test)]
mod api {
    use std::borrow::Cow;
    use std::error::Error;
    use std::ffi::{OsStr, OsString};
    use std::fmt::Display;
    use std::io;
    use std::iter::FusedIterator;
    use std::path::{Path, PathBuf};

    use snp_pycompat::chars::{self, PyChar};
    use snp_pycompat::codec::{self, FsDecodeChars, Utf8DecodeError, Utf8EncodeError, Utf8Reason};
    use snp_pycompat::int::{self, IntParseError};
    use snp_pycompat::ntpath::{self, Unit};
    use snp_pycompat::ospath::{self, HomeLookup};
    use snp_pycompat::precommit::{self, PrePushLine, PrePushLines, PrePushLinesBytes, ZSplit};
    use snp_pycompat::random::{self, Mt19937};
    use snp_pycompat::repr::{self, PyInt, PyValue};
    use snp_pycompat::shlex::{self, ShlexError};
    use snp_pycompat::text::{self, BytesSplitLines, SplitLines, SplitWs, UnpackError};
    use snp_pycompat::{PathError, eaw, float, posixpath, textio};

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }
    fn error_type<E: Error + Display + Clone + Send + Sync + 'static>() {}
    fn send_sync<T: Send + Sync>() {}
    fn str_iter<'a, I>()
    where
        I: Iterator<Item = &'a str> + FusedIterator + Clone,
    {
    }
    fn bytes_iter<'a, I>()
    where
        I: Iterator<Item = &'a [u8]> + FusedIterator + Clone,
    {
    }

    fn double_ended<I: DoubleEndedIterator>() {}
    fn chars_iter<I: Iterator<Item = PyChar> + Clone>() {}
    fn unit<U: Unit>() {}
    fn pp<'a, I: Iterator<Item = Result<PrePushLine<'a>, UnpackError>> + Clone>() {}
    fn ppb<'a, I: Iterator<Item = Result<[&'a [u8]; 4], UnpackError>> + Clone>() {}

    #[test]
    fn errors() {
        error_type::<PathError>();
        error_type::<UnpackError>();
        error_type::<Utf8DecodeError>();
        error_type::<Utf8EncodeError>();
        error_type::<IntParseError>();
        error_type::<ShlexError>();
        has::<fn(&PathError) -> &'static str>(PathError::py_type);
        has::<fn(&UnpackError) -> &'static str>(UnpackError::py_type);
        has::<fn(&Utf8DecodeError) -> &'static str>(Utf8DecodeError::py_type);
        has::<fn(&Utf8EncodeError) -> &'static str>(Utf8EncodeError::py_type);
        has::<fn(&IntParseError) -> &'static str>(IntParseError::py_type);
        has::<fn(&ShlexError) -> &'static str>(ShlexError::py_type);
        has::<[PathError; 5]>([
            PathError::NoPathSpecified,
            PathError::EmptySequence,
            PathError::MixAbsRel,
            PathError::DifferentDrives,
            PathError::DifferentMounts {
                path_drive: String::new(),
                start_drive: String::new(),
            },
        ]);
        has::<UnpackError>(UnpackError {
            expected: 4,
            got: 3,
        });
        has::<Utf8DecodeError>(Utf8DecodeError {
            start: 0,
            end: 1,
            first_byte: 0xff,
            reason: Utf8Reason::InvalidStartByte,
        });
        has::<[Utf8Reason; 3]>([
            Utf8Reason::InvalidStartByte,
            Utf8Reason::InvalidContinuationByte,
            Utf8Reason::UnexpectedEndOfData,
        ]);
        has::<Utf8EncodeError>(Utf8EncodeError {
            position: 0,
            surrogate: 0xdcff,
        });
        has::<IntParseError>(IntParseError {
            literal: String::new(),
        });
        has::<[ShlexError; 2]>([
            ShlexError::NoClosingQuotation,
            ShlexError::NoEscapedCharacter,
        ]);
    }

    #[test]
    fn chars_and_text() {
        has::<&str>(chars::UNICODE_VERSION);
        has::<&str>(chars::PY_STR_SPACE_CLASS);
        has::<fn(char) -> bool>(chars::is_space);
        has::<fn(char) -> bool>(chars::is_linebreak);
        has::<fn(u8) -> bool>(chars::is_bytes_space);
        has::<fn(char) -> bool>(chars::is_printable);
        has::<fn(char) -> Option<u32>>(chars::decimal_value);
        has::<[PyChar; 2]>([PyChar::Scalar('a'), PyChar::Surrogate(0xdcff)]);
        send_sync::<PyChar>();

        str_iter::<SplitLines<'static>>();
        double_ended::<SplitLines<'static>>();
        bytes_iter::<BytesSplitLines<'static>>();
        str_iter::<SplitWs<'static>>();
        has::<fn(&str) -> SplitLines<'_>>(text::splitlines);
        has::<fn(&str) -> SplitLines<'_>>(text::splitlines_keepends);
        has::<fn(&[u8]) -> BytesSplitLines<'_>>(text::bytes_splitlines);
        has::<fn(&[u8]) -> BytesSplitLines<'_>>(text::bytes_splitlines_keepends);
        has::<fn(&str, Option<usize>) -> SplitWs<'_>>(text::split_whitespace);
        has::<fn(&str, Option<usize>) -> Vec<&str>>(text::rsplit_whitespace);
        has::<fn(&[u8], Option<usize>) -> Vec<&[u8]>>(text::bytes_rsplit_whitespace);
        has::<fn(&str) -> Result<[&str; 4], UnpackError>>(text::rsplit4);
        has::<fn(&[u8]) -> Result<[&[u8]; 4], UnpackError>>(text::bytes_rsplit4);
        has::<fn(&str) -> &str>(text::strip);
        has::<fn(&str) -> &str>(text::lstrip);
        has::<fn(&str) -> &str>(text::rstrip);
        has::<for<'a> fn(&'a str, &str) -> &'a str>(text::strip_chars);
        has::<for<'a> fn(&'a str, &str) -> &'a str>(text::lstrip_chars);
        has::<for<'a> fn(&'a str, &str) -> &'a str>(text::rstrip_chars);
        has::<fn(&[u8]) -> &[u8]>(text::bytes_strip);
        has::<fn(&[u8]) -> &[u8]>(text::bytes_lstrip);
        has::<fn(&[u8]) -> &[u8]>(text::bytes_rstrip);
        has::<for<'a> fn(&'a [u8], &[u8]) -> &'a [u8]>(text::bytes_strip_set);
        has::<for<'a> fn(&'a [u8], &[u8]) -> &'a [u8]>(text::bytes_lstrip_set);
        has::<for<'a> fn(&'a [u8], &[u8]) -> &'a [u8]>(text::bytes_rstrip_set);
        has::<fn(&[u8]) -> &[u8]>(text::strip_fsbytes);
        has::<for<'a> fn(&'a str, &str) -> (&'a str, &'a str, &'a str)>(text::partition);
    }

    #[test]
    fn codec_repr_int_shlex() {
        has::<fn(&[u8]) -> Result<&str, Utf8DecodeError>>(codec::decode_utf8);
        has::<fn(&[u8]) -> Cow<'_, str>>(codec::decode_utf8_backslashreplace);
        chars_iter::<FsDecodeChars<'static>>();
        has::<fn(&[u8]) -> FsDecodeChars<'_>>(codec::fsdecode_chars);

        has::<fn(&str) -> String>(repr::repr_str);
        has::<fn(&mut String, &str)>(repr::write_repr_str);
        has::<fn(&[u8]) -> String>(repr::repr_fsbytes);
        has::<fn(&OsStr) -> String>(repr::repr_os);
        has::<fn(&[&'static str]) -> String>(repr::repr_str_tuple::<&'static str>);
        has::<fn(&[String]) -> String>(repr::repr_str_list::<String>);
        has::<[PyInt; 2]>([PyInt::Small(1), PyInt::Decimal("1".into())]);
        has::<PyValue>(PyValue::Dict(vec![(
            PyValue::None,
            PyValue::List(vec![
                PyValue::Bool(true),
                PyValue::Int(PyInt::Small(1)),
                PyValue::Float(1.0),
                PyValue::Str(String::new()),
                PyValue::Tuple(Vec::new()),
            ]),
        )]));
        has::<fn(&PyValue) -> String>(repr::repr);
        has::<fn(&mut String, &PyValue)>(repr::write_repr);
        has::<fn(f64) -> String>(repr::repr_float);
        has::<fn(&str, &str) -> String>(repr::fstring_eq);
        has::<fn(i32, &str, Option<&OsStr>, Option<&OsStr>) -> String>(repr::oserror_str);
        has::<fn(&io::Error, Option<&OsStr>, Option<&OsStr>) -> String>(repr::oserror_from_io);
        has::<fn(i64, &str) -> String>(repr::stdlib_called_process_error_str);
        has::<fn(u32) -> Option<&'static str>>(repr::signal_name);
        has::<fn(&str) -> String>(repr::int_literal_error);

        has::<fn(&str) -> Result<i128, IntParseError>>(int::parse_int);
        has::<fn(&str) -> Result<Vec<i128>, IntParseError>>(int::parse_version);

        has::<fn(&str) -> Cow<'_, str>>(shlex::quote);
        has::<fn(&[u8]) -> Cow<'_, [u8]>>(shlex::quote_bytes);
        has::<fn(Vec<&'static str>) -> String>(shlex::join::<Vec<&'static str>, &'static str>);
        has::<fn(&str) -> Result<Vec<String>, ShlexError>>(shlex::split);
    }

    #[test]
    fn paths() {
        has::<fn(&[u8]) -> Vec<u8>>(posixpath::normpath);
        has::<fn(&[u8], &[&[u8]]) -> Vec<u8>>(posixpath::join);
        has::<fn(&[u8]) -> (&[u8], &[u8])>(posixpath::split);
        has::<fn(&[u8]) -> &[u8]>(posixpath::dirname);
        has::<fn(&[u8]) -> &[u8]>(posixpath::basename);
        has::<fn(&[u8]) -> (&[u8], &[u8])>(posixpath::splitext);
        has::<fn(&[u8]) -> bool>(posixpath::isabs);
        has::<fn(&[u8], &[u8]) -> Vec<u8>>(posixpath::abspath);
        has::<fn(&[u8], Option<&[u8]>, &[u8]) -> Result<Vec<u8>, PathError>>(posixpath::relpath);
        has::<fn(&[&[u8]]) -> Result<Vec<u8>, PathError>>(posixpath::commonpath);
        has::<
            for<'a> fn(
                &'a [u8],
                Option<&[u8]>,
                &dyn Fn() -> Option<Vec<u8>>,
                &dyn Fn(&[u8]) -> Option<Vec<u8>>,
            ) -> Cow<'a, [u8]>,
        >(posixpath::expanduser);
        #[cfg(unix)]
        has::<fn(&[u8], &[u8]) -> io::Result<Vec<u8>>>(posixpath::realpath);

        unit::<u8>();
        unit::<u16>();
        has::<fn(u8) -> u16>(<u16 as Unit>::from_ascii);
        has::<fn(u16) -> Option<u8>>(<u16 as Unit>::to_ascii);
        has::<fn(&[u16]) -> (&[u16], &[u16], &[u16])>(ntpath::splitroot::<u16>);
        has::<fn(&[u16]) -> (&[u16], &[u16])>(ntpath::splitdrive::<u16>);
        has::<fn(&[u16]) -> bool>(ntpath::isabs_312::<u16>);
        has::<fn(&[u16]) -> bool>(ntpath::isabs_313::<u16>);
        has::<fn(&[u16]) -> Vec<u16>>(ntpath::normpath::<u16>);
        has::<fn(&[u16], &[&[u16]]) -> Vec<u16>>(ntpath::join::<u16>);
        has::<fn(&[u16]) -> (Vec<u16>, &[u16])>(ntpath::split::<u16>);
        has::<fn(&[u16]) -> Vec<u16>>(ntpath::dirname::<u16>);
        has::<fn(&[u16]) -> &[u16]>(ntpath::basename::<u16>);
        has::<fn(&[u16]) -> (&[u16], &[u16])>(ntpath::splitext::<u16>);
        has::<fn(&[u16]) -> Vec<u16>>(ntpath::normcase::<u16>);
        has::<fn(&[&[u16]]) -> Result<Vec<u16>, PathError>>(ntpath::commonpath::<u16>);
        has::<fn(&[u16], &[u16]) -> Vec<u16>>(ntpath::abspath_lexical::<u16>);
        has::<fn(&[u16], Option<&[u16]>, &[u16]) -> Result<Vec<u16>, PathError>>(
            ntpath::relpath::<u16>,
        );
        has::<for<'a> fn(&'a [u8], &dyn Fn(&str) -> Option<Vec<u8>>) -> Cow<'a, [u8]>>(
            ntpath::expanduser::<u8>,
        );
        #[cfg(windows)]
        has::<fn(&OsStr, &Path) -> io::Result<PathBuf>>(ntpath::realpath);

        let env = |_: &str| None;
        let pw_current = || None;
        let pw_named = |_: &OsStr| None;
        let home = HomeLookup {
            env: &env,
            pw_current: &pw_current,
            pw_named: &pw_named,
        };
        has::<Cow<'_, OsStr>>(ospath::expanduser(OsStr::new("~"), &home));
        has::<String>(format!("{home:?}"));
        has::<fn(&OsStr) -> OsString>(ospath::normpath);
        has::<fn(&OsStr, &[&OsStr]) -> OsString>(ospath::join);
        has::<fn(&OsStr) -> (OsString, OsString)>(ospath::split);
        has::<fn(&OsStr) -> OsString>(ospath::dirname);
        has::<fn(&OsStr) -> OsString>(ospath::basename);
        has::<fn(&OsStr) -> bool>(ospath::isabs);
        has::<fn(&OsStr, &Path) -> PathBuf>(ospath::abspath);
        has::<fn(&OsStr, Option<&OsStr>, &Path) -> Result<OsString, PathError>>(ospath::relpath);
        has::<fn(&[&OsStr]) -> Result<OsString, PathError>>(ospath::commonpath);
        has::<fn(&OsStr) -> OsString>(ospath::normcase);
        has::<fn(&OsStr, &Path) -> io::Result<PathBuf>>(ospath::realpath);

        has::<fn(&str) -> Cow<'_, str>>(textio::universal_newlines);
        has::<fn(&str) -> Cow<'_, str>>(textio::windows_text_write);
        has::<fn(&str, &str) -> String>(textio::textwrap_indent);
    }

    #[test]
    fn precommit_random_eaw_float() {
        bytes_iter::<ZSplit<'static>>();
        has::<fn(&[u8]) -> ZSplit<'_>>(precommit::zsplit);
        has::<fn(Option<&OsStr>) -> Vec<OsString>>(precommit::get_skips);
        has::<PrePushLine<'_>>(PrePushLine {
            local_branch: "",
            local_sha: "",
            remote_branch: "",
            remote_sha: "",
        });
        pp::<PrePushLines<'static>>();
        ppb::<PrePushLinesBytes<'static>>();
        has::<fn(&str) -> PrePushLines<'_>>(precommit::pre_push_lines);
        has::<fn(&[u8]) -> PrePushLinesBytes<'_>>(precommit::pre_push_lines_bytes);
        has::<fn(&[u8]) -> Vec<&[u8]>>(precommit::merge_msg_conflicts);
        has::<fn(&str) -> Option<[&str; 6]>>(precommit::rev_line_match);
        has::<fn(&str, &str, Option<&str>) -> Option<String>>(precommit::rev_line_rewrite);
        has::<fn(&str) -> (&str, &str)>(precommit::migrate_header_split);
        has::<fn(&OsStr, &OsStr) -> bool>(precommit::exe_exists_pure);

        has::<fn(&[u32]) -> Mt19937>(Mt19937::init_by_array);
        has::<fn(&mut Mt19937) -> u32>(Mt19937::genrand_u32);
        has::<fn(&mut Mt19937, u64) -> u64>(Mt19937::randbelow);
        has::<fn(usize) -> Vec<u32>>(random::precommit_shuffled_indices);
        send_sync::<Mt19937>();

        has::<fn(&str) -> usize>(eaw::len_cjk);
        has::<fn(f64) -> String>(float::repr);
        has::<fn(f64) -> String>(float::format_duration);
    }
}
