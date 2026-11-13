//! Engine construction and matching (01 Addendum §A.6.9–§A.6.12, ADR-079).
//!
//! * Tier 1: `regex::Regex` (str) / `regex::bytes::Regex` with `unicode(false)` (bytes), `size_limit` 64 MiB,
//!   `dfa_size_limit` the crate default 2 MiB (the per-regex per-thread lazy-DFA cache, ADR-079),
//!   `nest_limit` 2048. A haystack that meets a condition bit of the fast text uses the exact variant, which
//!   runs on `fancy_regex` behind a relaxed tier-1 [`Prefilter`] when the condition is a word boundary or a
//!   nullable-loop span (ADR-097): the prefilter is built on the first haystack that needs the exact variant
//!   (eagerly with an eager exact fallback) with a 256 KiB lazy-DFA cache.
//! * Tier 2: `fancy_regex` with `backtrack_limit` 10⁸, delegate limits 64 MiB / 2 MiB, always behind the
//!   relaxed tier-1 prefilter; bytes patterns run on the Latin-1 lane (each byte `b` becomes `char::from(b)`).
//!
//! A prefilter that could not be built, or that would match every haystack, is simply absent: it only ever
//! turns a haystack the relaxed text rejects into a no-match (relaxed ⊇ exact, §A.6.11), never into an error.
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
/// `dfa_size_limit` of the tier-1 exact fallback's prefilter (ADR-097): it runs only on the haystacks that meet a
/// condition bit, and a full 2 MiB cache per pattern per worker thread would cost B12u ~5 MiB of RSS.
const PREFILTER_DFA_SIZE_LIMIT: usize = 256 << 10;
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
        Self::build_limited(text, bytes, depth, DFA_SIZE_LIMIT)
    }

    /// [`T1::build`] with a lazy-DFA cache capacity of `dfa_size_limit`.
    fn build_limited(
        text: &str,
        bytes: bool,
        depth: u32,
        dfa_size_limit: usize,
    ) -> Result<Self, String> {
        on_stack(depth, || Self::build_here(text, bytes, dfa_size_limit))
            .unwrap_or_else(|| Err(messages::NO_BUILD_THREAD.into()))
    }

    fn build_here(text: &str, bytes: bool, dfa_size_limit: usize) -> Result<Self, String> {
        if bytes {
            regex::bytes::RegexBuilder::new(text)
                .unicode(false)
                .size_limit(SIZE_LIMIT)
                .dfa_size_limit(dfa_size_limit)
                .nest_limit(NEST_LIMIT)
                .build()
                .map(Self::Bytes)
                .map_err(|e| describe_regex_error(&e))
        } else {
            regex::RegexBuilder::new(text)
                .size_limit(SIZE_LIMIT)
                .dfa_size_limit(dfa_size_limit)
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

/// A relaxed tier-1 prefilter in front of a backtracking engine (§A.6.11, ADR-097): a haystack it rejects
/// has no match, because the relaxed text's language is a superset of the exact text's.
#[derive(Debug)]
pub(crate) struct Prefilter(T1);

impl Prefilter {
    /// The prefilter of the relaxed `text`, or `None` when it cannot be built or is useless: a text without
    /// anchors that matches the empty string matches every haystack.
    fn build(
        text: &str,
        bytes: bool,
        depth: u32,
        anchored: bool,
        dfa_size_limit: usize,
    ) -> Option<Self> {
        T1::build_limited(text, bytes, depth, dfa_size_limit)
            .ok()
            .filter(|r| anchored || !r.is_match(Hay::Bytes(b"")))
            .map(Self)
    }

    /// `false`: the haystack has no match of the exact text (under search, `re.match` and, on the fullmatch
    /// engines, `re.fullmatch` semantics alike).
    fn admits(&self, h: Hay<'_>) -> bool {
        self.0.is_match(h)
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
        #[cfg(test)]
        probe::count_exact_run();
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
        #[cfg(test)]
        probe::count_exact_run();
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
        #[cfg(test)]
        probe::count_exact_run();
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
    /// The relaxed prefilter text: tier 2, and tier 1 when the exact fallback serves a word-boundary, `\B` or
    /// span condition (ADR-097; not for a non-MULTILINE `$` alone).
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

/// The lazily built exact fallback of a tier-1 pattern and its relaxed prefilter.
#[derive(Debug)]
pub(crate) struct Fallback {
    /// Exact fallback text.
    exact_src: Option<Box<str>>,
    /// Exact fallback engine, built on first need.
    exact: OnceLock<Option<T2>>,
    /// Relaxed prefilter text (ADR-097).
    relaxed_src: Option<Box<str>>,
    /// The relaxed text has anchors.
    relaxed_anchored: bool,
    /// The prefilter, built on first need (`None`: none could be built, or it would match everywhere).
    relaxed: OnceLock<Option<Prefilter>>,
    /// Depth of the texts.
    depth: u32,
    /// Bytes pattern.
    bytes: bool,
}

impl Fallback {
    /// The exact fallback (`None` if it could not be built).
    fn exact(&self) -> Option<&T2> {
        self.exact
            .get_or_init(|| {
                self.exact_src
                    .as_deref()
                    .and_then(|s| T2::build(s, self.bytes, self.depth).ok())
            })
            .as_ref()
    }

    /// The prefilter of the exact fallback, if it has a useful one.
    fn prefilter(&self) -> Option<&Prefilter> {
        #[cfg(test)]
        if probe::prefilter_off() {
            return None;
        }
        self.relaxed
            .get_or_init(|| {
                self.relaxed_src.as_deref().and_then(|s| {
                    Prefilter::build(
                        s,
                        self.bytes,
                        self.depth,
                        self.relaxed_anchored,
                        PREFILTER_DFA_SIZE_LIMIT,
                    )
                })
            })
            .as_ref()
    }

    /// `false`: the prefilter rejects `h`, so the exact text has no match on it.
    fn admits(&self, h: Hay<'_>) -> bool {
        self.prefilter().is_none_or(|p| p.admits(h))
    }
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
        /// The exact fallback and its prefilter.
        fallback: Box<Fallback>,
    },
    /// Tier 2.
    T2 {
        /// The prefilter (absent when it would match everywhere or could not be built).
        relaxed: Option<Prefilter>,
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
pub(crate) fn full(text: &str) -> String {
    format!("\\A(?:{text})\\z")
}

impl Compiled {
    /// Builds the engines of `texts` (`full`: the `re.fullmatch` variant); `eager_exact` also builds the
    /// exact fallback (so that an engine limit is a compile error) and its prefilter now.
    pub(crate) fn build(
        texts: &Texts,
        full_match: bool,
        eager_exact: bool,
    ) -> Result<Self, String> {
        let wrap = |t: &str| if full_match { full(t) } else { t.to_owned() };
        let depth = texts.depth.saturating_add(u32::from(full_match));
        if texts.tier2 {
            let exact = T2::build(&wrap(&texts.main), texts.bytes, depth)?;
            let relaxed = texts.relaxed.as_deref().and_then(|r| {
                Prefilter::build(
                    &wrap(r),
                    texts.bytes,
                    depth,
                    texts.relaxed_anchored,
                    DFA_SIZE_LIMIT,
                )
            });
            return Ok(Self::T2 { relaxed, exact });
        }
        let fast = T1::build(&wrap(&texts.main), texts.bytes, depth)?;
        let fallback = Fallback {
            exact_src: texts.exact.as_deref().map(|e| wrap(e).into_boxed_str()),
            exact: OnceLock::new(),
            relaxed_src: texts.relaxed.as_deref().map(|r| wrap(r).into_boxed_str()),
            relaxed_anchored: texts.relaxed_anchored,
            relaxed: OnceLock::new(),
            depth,
            bytes: texts.bytes,
        };
        if eager_exact && let Some(src) = &fallback.exact_src {
            let built = T2::build(src, texts.bytes, depth)?;
            fallback.exact.set(Some(built)).unwrap_or_default();
            // A prefilter that cannot be built is no error: this only moves its build out of the match path.
            _ = fallback.prefilter();
        }
        Ok(Self::T1 {
            fast,
            cond: texts.cond,
            fallback: Box::new(fallback),
        })
    }

    /// `re.search(...) is not None` (`Op::Search`, and `Op::Full` on the fullmatch engines) or `re.match(...)
    /// is not None` (`Op::Prefix`).
    pub(crate) fn is_match(&self, h: Hay<'_>, op: Op) -> Result<bool, MatchError> {
        let prefix = op == Op::Prefix;
        match self {
            Self::T1 {
                fast,
                cond,
                fallback,
            } => {
                if cond.needs_exact(h.bytes()) {
                    if !fallback.admits(h) {
                        return Ok(false);
                    }
                    let Some(e) = fallback.exact() else {
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
                    && !r.admits(h)
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
                fallback,
            } => {
                if cond.needs_exact_span(h.bytes()) {
                    if !fallback.admits(h) {
                        return Ok(None);
                    }
                    let Some(e) = fallback.exact() else {
                        return Err(MatchError::Backtrack);
                    };
                    return e.find(h);
                }
                Ok(fast.find(h))
            }
            Self::T2 { relaxed, exact } => {
                if let Some(r) = relaxed
                    && !r.admits(h)
                {
                    return Ok(None);
                }
                exact.find(h)
            }
        }
    }
}

/// Test-only probes of the backtracking path (thread-local: the unit tests run on parallel threads).
#[cfg(test)]
pub(crate) mod probe {
    use std::cell::Cell;

    thread_local! {
        static EXACT_RUNS: Cell<u64> = const { Cell::new(0) };
        static PREFILTER_OFF: Cell<bool> = const { Cell::new(false) };
    }

    /// One run of a backtracking engine.
    pub(super) fn count_exact_run() {
        EXACT_RUNS.with(|c| c.set(c.get().saturating_add(1)));
    }

    /// The backtracking-engine runs on this thread so far.
    pub(crate) fn exact_runs() -> u64 {
        EXACT_RUNS.with(Cell::get)
    }

    /// Whether the tier-1 exact fallback's prefilter is switched off on this thread.
    pub(super) fn prefilter_off() -> bool {
        PREFILTER_OFF.with(Cell::get)
    }

    /// Switches the tier-1 exact fallback's prefilter off (`true`: the pre-ADR-097 behaviour, the reference of
    /// the parity tests) or back on, for this thread.
    pub(crate) fn set_prefilter_off(off: bool) {
        PREFILTER_OFF.with(|c| c.set(off));
    }
}
