//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.5 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-13 (created by WP-00).

#[cfg(test)]
mod api {
    use std::fmt::Display;
    use std::sync::Arc;

    use snp_pyre::{
        Haystack, MatchError, MatchKind, Mode, Options, PyReError, PyRegex, PyWarning, RegexCache,
    };

    fn send_sync<T: Send + Sync>() {}
    fn display<T: Display>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Mode>();
        send_sync::<Haystack>();
        send_sync::<Options>();
        send_sync::<PyReError>();
        send_sync::<PyWarning>();
        send_sync::<MatchKind>();
        send_sync::<MatchError>();
        send_sync::<PyRegex>();
        send_sync::<RegexCache>();
        display::<PyReError>();
    }

    #[test]
    fn types() {
        let Options {
            mode,
            haystack,
            ignore_case,
            multiline_dotall,
        }: Options = Options::CONFIG;
        has::<(Mode, Haystack, bool, bool)>((mode, haystack, ignore_case, multiline_dotall));
        has::<[Mode; 2]>([Mode::Str, Mode::Bytes]);
        has::<[Haystack; 3]>([Haystack::Path, Haystack::Line, Haystack::Blob]);
        has::<[PyReError; 5]>([
            PyReError::Re {
                msg: "missing ), unterminated subpattern".into(),
                pos: Some(0),
            },
            PyReError::Overflow,
            PyReError::Value("ASCII and UNICODE flags are incompatible".into()),
            PyReError::Recursion,
            PyReError::Unsupported("x".into()),
        ]);
        has::<fn(&PyReError) -> &'static str>(PyReError::py_type);
        has::<fn(&PyReError) -> String>(PyReError::py_last_line);
        has::<PyWarning>(PyWarning {
            msg: "Possible nested set at position 1".into(),
        });
        has::<[MatchKind; 7]>([
            MatchKind::All,
            MatchKind::EmptyOrNewline,
            MatchKind::Prefix,
            MatchKind::Suffix,
            MatchKind::Contains,
            MatchKind::Tier1,
            MatchKind::Tier2,
        ]);
        has::<MatchError>(MatchError::Backtrack);
        has::<RegexCache>(RegexCache::default());
    }

    #[test]
    fn functions() {
        has::<fn(&str, Mode) -> Result<Vec<PyWarning>, PyReError>>(snp_pyre::validate);
        has::<fn(&str) -> bool>(snp_pyre::cfgv_is_valid);
        has::<fn(&str, Options) -> Result<PyRegex, PyReError>>(PyRegex::compile);
        has::<fn(&PyRegex) -> &str>(PyRegex::source);
        has::<fn(&PyRegex) -> MatchKind>(PyRegex::kind);
        has::<fn(&PyRegex) -> &[PyWarning]>(PyRegex::warnings);
        has::<fn(&PyRegex, &str) -> Result<bool, MatchError>>(PyRegex::search);
        has::<fn(&PyRegex, &str) -> Result<bool, MatchError>>(PyRegex::match_prefix);
        has::<fn(&PyRegex, &str) -> Result<bool, MatchError>>(PyRegex::fullmatch);
        has::<fn(&PyRegex, &[u8]) -> Result<Option<(usize, usize)>, MatchError>>(
            PyRegex::search_bytes,
        );
        has::<fn() -> RegexCache>(RegexCache::new);
        has::<fn(&RegexCache, &str, Options) -> Result<Arc<PyRegex>, PyReError>>(RegexCache::get);
    }
}
