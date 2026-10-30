//! Engine construction and matching (01 Addendum §A.6.9–§A.6.12, ADR-079).
//!
//! * Tier 1: `regex::Regex` (str) / `regex::bytes::Regex` with `unicode(false)` (bytes), `size_limit` 64 MiB,
//!   `dfa_size_limit` the crate default 2 MiB (the per-regex per-thread lazy-DFA cache, ADR-079),
//!   `nest_limit` 2048. A haystack that meets a condition bit of the fast text uses the exact variant.
//! * Tier 2: `fancy_regex` with `backtrack_limit` 10⁸, delegate limits 64 MiB / 2 MiB, always behind the
//!   relaxed tier-1 prefilter; bytes patterns run on the Latin-1 lane (each byte `b` becomes `char::from(b)`).
//!
//! Engines of deeply nested patterns are built on a helper thread with a large stack: `regex-automata`'s
//! compiler recurses once per nesting level, and snp's threads (and the fuzzers' 256 KiB stacks) are small.
//!
//! Owner: WP-13.

use std::borrow::Cow;
use std::sync::OnceLock;

use crate::error::MatchError;
use crate::lower::Cond;
use crate::messages;

/// `size_limit` of every `regex` build (01 Addendum §A.6.10).
const SIZE_LIMIT: usize = 64 << 20;
/// `dfa_size_limit`: the crate default, the lazy-DFA cache capacity per regex per thread (ADR-079).
const DFA_SIZE_LIMIT: usize = 2 << 20;
/// `nest_limit` of tier 1 (Python's 484 nested groups lower to well under 2048 levels).
const NEST_LIMIT: u32 = 2048;
/// fancy-regex `backtrack_limit` (§A.1 item 10).
const BACKTRACK_LIMIT: usize = 100_000_000;
/// Deepest group nesting fancy-regex parses (its `MAX_RECURSION` is 64: depth 64 fails).
pub(crate) const FANCY_MAX_DEPTH: u32 = 63;
/// Emitted nesting above which engines are built on a helper thread.
const DEEP: u32 = 48;
/// The helper thread's stack.
const DEEP_STACK: usize = 256 << 20;

/// A haystack as the caller passed it.
#[derive(Copy, Clone, Debug)]
pub(crate) enum Hay<'a> {
    /// A `&str` (paths).
    Str(&'a str),
    /// Raw bytes (pygrep lines and files).
    Bytes(&'a [u8]),
}

impl<'a> Hay<'a> {
    /// The bytes of the haystack.
    pub(crate) const fn bytes(self) -> &'a [u8] {
        match self {
            Self::Str(s) => s.as_bytes(),
            Self::Bytes(b) => b,
        }
    }

    /// The haystack as text (`None` for bytes that are not UTF-8: a str pattern never matches them).
    pub(crate) fn text(self) -> Option<&'a str> {
        match self {
            Self::Str(s) => Some(s),
            Self::Bytes(b) => std::str::from_utf8(b).ok(),
        }
    }
}

/// A tier-1 engine.
#[derive(Debug)]
pub(crate) enum T1 {
    /// str pattern.
    Str(regex::Regex),
    /// bytes pattern (`unicode(false)`).
    Bytes(regex::bytes::Regex),
}

impl T1 {
    /// Builds `text` (on a large stack when `depth` is deep).
    pub(crate) fn build(text: &str, bytes: bool, depth: u32) -> Result<Self, String> {
        on_stack(depth, || Self::build_here(text, bytes))
            .unwrap_or_else(|| Err(messages::NO_BUILD_THREAD.into()))
    }

    fn build_here(text: &str, bytes: bool) -> Result<Self, String> {
        if bytes {
            regex::bytes::RegexBuilder::new(text)
                .unicode(false)
                .size_limit(SIZE_LIMIT)
                .dfa_size_limit(DFA_SIZE_LIMIT)
                .nest_limit(NEST_LIMIT)
                .build()
                .map(Self::Bytes)
                .map_err(|e| describe_regex_error(&e))
        } else {
            regex::RegexBuilder::new(text)
                .size_limit(SIZE_LIMIT)
                .dfa_size_limit(DFA_SIZE_LIMIT)
                .nest_limit(NEST_LIMIT)
                .build()
                .map(Self::Str)
                .map_err(|e| describe_regex_error(&e))
        }
    }

    /// `is_match` on the haystack (str engines never match non-UTF-8 bytes).
    pub(crate) fn is_match(&self, h: Hay<'_>) -> bool {
        match self {
            Self::Str(re) => h.text().is_some_and(|t| re.is_match(t)),
            Self::Bytes(re) => re.is_match(h.bytes()),
        }
    }

    /// The leftmost-first match span in bytes.
    pub(crate) fn find(&self, h: Hay<'_>) -> Option<(usize, usize)> {
        match self {
            Self::Str(re) => h
                .text()
                .and_then(|t| re.find(t))
                .map(|m| (m.start(), m.end())),
            Self::Bytes(re) => re.find(h.bytes()).map(|m| (m.start(), m.end())),
        }
    }
}

/// A tier-2 engine.
#[derive(Debug)]
pub(crate) struct T2 {
    re: fancy_regex::Regex,
    /// bytes pattern on the Latin-1 lane.
    latin1: bool,
}

impl T2 {
    /// Builds `text` (on a large stack when `depth` is deep).
    pub(crate) fn build(text: &str, bytes: bool, depth: u32) -> Result<Self, String> {
        if depth > FANCY_MAX_DEPTH {
            return Err(messages::TOO_DEEP_FOR_TIER2.into());
        }
        on_stack(depth, || {
            fancy_regex::RegexBuilder::new(text)
                .backtrack_limit(BACKTRACK_LIMIT)
                .delegate_size_limit(SIZE_LIMIT)
                .delegate_dfa_size_limit(DFA_SIZE_LIMIT)
                .build()
                .map(|re| Self { re, latin1: bytes })
                .map_err(|e| messages::engine_limit(&e.to_string()))
        })
        .unwrap_or_else(|| Err(messages::NO_BUILD_THREAD.into()))
    }

    /// `is_match` (`MatchError::Backtrack` past the backtrack limit).
    pub(crate) fn is_match(&self, h: Hay<'_>) -> Result<bool, MatchError> {
        if self.latin1 {
            let lane = latin1(h.bytes());
            return self
                .re
                .is_match(lane.as_ref())
                .map_err(|_| MatchError::Backtrack);
        }
        match h.text() {
            Some(t) => self.re.is_match(t).map_err(|_| MatchError::Backtrack),
            None => Ok(false),
        }
    }

    /// A match starting at position 0 (an anchored search: no other start position is tried).
    pub(crate) fn matches_at_start(&self, h: Hay<'_>) -> Result<bool, MatchError> {
        let anchored = |t: &str| {
            self.re
                .find_input(fancy_regex::RegexInput::new(t).anchored(true))
                .map(|m| m.is_some())
                .map_err(|_| MatchError::Backtrack)
        };
        if self.latin1 {
            return anchored(latin1(h.bytes()).as_ref());
        }
        h.text().map_or(Ok(false), anchored)
    }

    /// The leftmost-first match span in bytes of the original haystack.
    pub(crate) fn find(&self, h: Hay<'_>) -> Result<Option<(usize, usize)>, MatchError> {
        if self.latin1 {
            let lane = latin1(h.bytes());
            let m = self
                .re
                .find(lane.as_ref())
                .map_err(|_| MatchError::Backtrack)?;
            return Ok(m.map(|m| match &lane {
                // ASCII: the lane is the haystack itself.
                Cow::Borrowed(_) => (m.start(), m.end()),
                Cow::Owned(lane) => (char_offset(lane, m.start()), char_offset(lane, m.end())),
            }));
        }
        match h.text() {
            Some(t) => Ok(self
                .re
                .find(t)
                .map_err(|_| MatchError::Backtrack)?
                .map(|m| (m.start(), m.end()))),
            None => Ok(None),
        }
    }
}

/// The Latin-1 lane: each byte becomes the character with the same value (a bijection onto U+0000–U+00FF).
/// ASCII haystacks (the usual pygrep line) are their own lane and are not copied.
fn latin1(b: &[u8]) -> Cow<'_, str> {
    if b.is_ascii()
        && let Ok(s) = std::str::from_utf8(b)
    {
        return Cow::Borrowed(s);
    }
    Cow::Owned(b.iter().copied().map(char::from).collect())
}

/// A byte offset of the Latin-1 lane mapped back to the original bytes (one character per byte).
fn char_offset(lane: &str, offset: usize) -> usize {
    lane.get(..offset).map_or(offset, |s| s.chars().count())
}

/// The message of a `regex` build error (a size limit; a syntax error would be a lowering bug).
fn describe_regex_error(e: &regex::Error) -> String {
    match e {
        regex::Error::CompiledTooBig(limit) => {
            messages::engine_limit(&format!("compiled size exceeds {limit} bytes"))
        }
        other => messages::engine_limit(&other.to_string()),
    }
}

/// Runs `f` here, or on a helper thread with a large stack when `depth` is deep (`None`: no thread).
fn on_stack<T: Send>(depth: u32, f: impl FnOnce() -> T + Send) -> Option<T> {
    if depth <= DEEP {
        return Some(f());
    }
    std::thread::scope(|s| {
        let handle = std::thread::Builder::new()
            .name("snp-pyre-build".into())
            .stack_size(DEEP_STACK)
            .spawn_scoped(s, f)
            .ok()?;
        handle.join().ok()
    })
}

/// The texts a pattern lowers to (kept for the lazily built variants).
#[derive(Debug, Clone)]
pub(crate) struct Texts {
    /// Tier 1: the fast text; tier 2: the exact text.
    pub(crate) main: Box<str>,
    /// Tier 1: the exact fallback text (when a condition bit is set).
    pub(crate) exact: Option<Box<str>>,
    /// Tier 2: the relaxed prefilter text.
    pub(crate) relaxed: Option<Box<str>>,
    /// Tier 2.
    pub(crate) tier2: bool,
    /// Condition bits of the fast text.
    pub(crate) cond: Cond,
    /// Deepest nesting of the texts.
    pub(crate) depth: u32,
    /// Bytes pattern.
    pub(crate) bytes: bool,
    /// The relaxed text has anchors (the prefilter is useful even if it matches the empty string).
    pub(crate) relaxed_anchored: bool,
}

/// A compiled set of engines for one variant of the texts (`P` for `re.search`/`re.match`, `\A(?:P)\z` for
/// `re.fullmatch`).
#[derive(Debug)]
pub(crate) enum Compiled {
    /// Tier 1.
    T1 {
        /// Fast engine.
        fast: T1,
        /// Condition bits.
        cond: Cond,
        /// Exact fallback text.
        exact_src: Option<Box<str>>,
        /// Exact fallback engine, built on first need.
        exact: OnceLock<Option<T2>>,
        /// Depth of the exact text.
        depth: u32,
        /// Bytes pattern.
        bytes: bool,
    },
    /// Tier 2.
    T2 {
        /// The prefilter (absent when it would match everywhere or could not be built).
        relaxed: Option<T1>,
        /// The backtracking engine.
        exact: T2,
    },
}

/// What a match call asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// `re.search`.
    Search,
    /// `re.match`: a match starting at 0 (the leftmost match of the search engines, or an anchored search).
    Prefix,
    /// `re.fullmatch`: the separately built `\A(?:P)\z`.
    Full,
}

/// `\A(?:P)\z` for the `re.fullmatch` engines.
fn full(text: &str) -> String {
    format!("\\A(?:{text})\\z")
}

impl Compiled {
    /// Builds the engines of `texts` (`full`: the `re.fullmatch` variant); `eager_exact` also builds the
    /// exact fallback now.
    pub(crate) fn build(
        texts: &Texts,
        full_match: bool,
        eager_exact: bool,
    ) -> Result<Self, String> {
        let wrap = |t: &str| if full_match { full(t) } else { t.to_owned() };
        let depth = texts.depth.saturating_add(u32::from(full_match));
        if texts.tier2 {
            let exact = T2::build(&wrap(&texts.main), texts.bytes, depth)?;
            let relaxed = texts
                .relaxed
                .as_deref()
                .and_then(|r| T1::build(&wrap(r), texts.bytes, depth).ok());
            let relaxed =
                relaxed.filter(|r| texts.relaxed_anchored || !r.is_match(Hay::Bytes(b"")));
            return Ok(Self::T2 { relaxed, exact });
        }
        let fast = T1::build(&wrap(&texts.main), texts.bytes, depth)?;
        let exact_src = texts.exact.as_deref().map(|e| wrap(e).into_boxed_str());
        let exact = OnceLock::new();
        if eager_exact && let Some(src) = &exact_src {
            let built = T2::build(src, texts.bytes, depth)?;
            exact.set(Some(built)).unwrap_or_default();
        }
        Ok(Self::T1 {
            fast,
            cond: texts.cond,
            exact_src,
            exact,
            depth,
            bytes: texts.bytes,
        })
    }

    /// The exact fallback of a tier-1 pattern (`None` if it could not be built).
    fn exact<'a>(
        cell: &'a OnceLock<Option<T2>>,
        src: Option<&str>,
        bytes: bool,
        depth: u32,
    ) -> Option<&'a T2> {
        cell.get_or_init(|| src.and_then(|s| T2::build(s, bytes, depth).ok()))
            .as_ref()
    }

    /// `re.search(...) is not None` (`Op::Search`, and `Op::Full` on the fullmatch engines) or `re.match(...)
    /// is not None` (`Op::Prefix`).
    pub(crate) fn is_match(&self, h: Hay<'_>, op: Op) -> Result<bool, MatchError> {
        let prefix = op == Op::Prefix;
        match self {
            Self::T1 {
                fast,
                cond,
                exact_src,
                exact,
                depth,
                bytes,
            } => {
                if cond.needs_exact(h.bytes()) {
                    let Some(e) = Self::exact(exact, exact_src.as_deref(), *bytes, *depth) else {
                        return Err(MatchError::Backtrack);
                    };
                    return if prefix {
                        e.matches_at_start(h)
                    } else {
                        e.is_match(h)
                    };
                }
                // The leftmost match starts at 0 iff some match does.
                Ok(if prefix {
                    fast.find(h).is_some_and(|(s, _)| s == 0)
                } else {
                    fast.is_match(h)
                })
            }
            Self::T2 { relaxed, exact } => {
                if let Some(r) = relaxed
                    && !r.is_match(h)
                {
                    return Ok(false);
                }
                if prefix {
                    exact.matches_at_start(h)
                } else {
                    exact.is_match(h)
                }
            }
        }
    }

    /// The leftmost-first match span (bytes).
    pub(crate) fn find(&self, h: Hay<'_>) -> Result<Option<(usize, usize)>, MatchError> {
        match self {
            Self::T1 {
                fast,
                cond,
                exact_src,
                exact,
                depth,
                bytes,
            } => {
                if cond.needs_exact_span(h.bytes()) {
                    let Some(e) = Self::exact(exact, exact_src.as_deref(), *bytes, *depth) else {
                        return Err(MatchError::Backtrack);
                    };
                    return e.find(h);
                }
                Ok(fast.find(h))
            }
            Self::T2 { relaxed, exact } => {
                if let Some(r) = relaxed
                    && !r.is_match(h)
                {
                    return Ok(None);
                }
                exact.find(h)
            }
        }
    }
}
