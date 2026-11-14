//! Engine construction and matching (01 Addendum §A.6.9–§A.6.12, ADR-079).
//!
//! * Tier 1: `regex::Regex` (str) / `regex::bytes::Regex` with `unicode(false)` (bytes), `size_limit` 64 MiB,
//!   `dfa_size_limit` the crate default 2 MiB (the per-regex per-thread lazy-DFA cache, ADR-079),
//!   `nest_limit` 2048. A haystack that meets a condition bit of the fast text uses the exact variant, which
//!   runs on `fancy_regex` behind a relaxed tier-1 [`Prefilter`] when the condition is a word boundary or a
//!   nullable-loop span (ADR-098): the prefilter is built on the first haystack that needs the exact variant
//!   (eagerly with an eager exact fallback). It is the forward-only `is_match` engine [`Forward`] (ADR-108): a
//!   lazy DFA with a 256 KiB cache over a forward NFA without capture states, falling back to the bounded
//!   backtracker or the `PikeVM` when the DFA cannot be built, gives up or quits.
//! * Tier 2: `fancy_regex` with `backtrack_limit` 10⁸, delegate limits 64 MiB / 2 MiB, always behind the
//!   relaxed tier-1 prefilter (`regex` with the crate-default 2 MiB cache); bytes patterns run on the Latin-1
//!   lane (each byte `b` becomes `char::from(b)`).
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

use regex_automata::nfa::thompson::{self, WhichCaptures, backtrack, pikevm};
use regex_automata::util::pool::Pool;
use regex_automata::util::syntax;
use regex_automata::{Input, hybrid};
use regex_syntax::hir::Look;

use crate::error::MatchError;
use crate::lower::Cond;
use crate::messages;

/// `size_limit` of every `regex` build (01 Addendum §A.6.10).
const SIZE_LIMIT: usize = 64 << 20;
/// `dfa_size_limit`: the crate default, the lazy-DFA cache capacity per regex per thread (ADR-079).
const DFA_SIZE_LIMIT: usize = 2 << 20;
/// The lazy-DFA cache capacity of the tier-1 exact fallback's prefilter (ADR-098, ADR-108): it runs only on the
/// haystacks that meet a condition bit, and a full 2 MiB cache per pattern per worker thread would cost B12u
/// ~5 MiB of RSS. A forward-only DFA fits it where `regex`'s forward + reverse pair did not (ADR-108).
pub(crate) const PREFILTER_DFA_SIZE_LIMIT: usize = 256 << 10;
/// Haystacks longer than this go to the `PikeVM` rather than the bounded backtracker when [`Forward`] falls back:
/// the backtracker cannot stop at the earliest match (regex-automata `meta`'s rule for `is_match`).
const BACKTRACK_MAX_HAYSTACK: usize = 128;
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

/// A relaxed tier-1 prefilter in front of a backtracking engine (§A.6.11, ADR-098): a haystack it rejects
/// has no match, because the relaxed text's language is a superset of the exact text's.
#[derive(Debug)]
pub(crate) struct Prefilter(Screen);

/// The engine of a [`Prefilter`].
#[derive(Debug)]
enum Screen {
    /// Tier 2's prefilter: `regex` with the crate-default 2 MiB lazy-DFA cache (ADR-079).
    Regex(T1),
    /// The tier-1 exact fallback's prefilter: the forward-only engine (ADR-108).
    Forward(Box<Forward>),
}

impl Prefilter {
    /// Tier 2's prefilter of the relaxed `text`, or `None` when it cannot be built or is useless.
    fn regex(text: &str, bytes: bool, depth: u32, anchored: bool) -> Option<Self> {
        let re = T1::build(text, bytes, depth).ok()?;
        Self(Screen::Regex(re)).useful(anchored)
    }

    /// The tier-1 exact fallback's prefilter of the relaxed `text` on the forward-only engine with a lazy-DFA
    /// cache of `cache_capacity` bytes, or `None` when its NFA cannot be built or it is useless. A lazy DFA that
    /// cannot be built at that capacity leaves the prefilter on the linear NFA engines, never without one.
    pub(crate) fn forward(
        text: &str,
        bytes: bool,
        depth: u32,
        anchored: bool,
        cache_capacity: usize,
    ) -> Option<Self> {
        let engine = Forward::build(text, bytes, depth, cache_capacity)?;
        Self(Screen::Forward(Box::new(engine))).useful(anchored)
    }

    /// `self`, unless it is useless: a text without anchors that matches the empty string matches every
    /// haystack.
    fn useful(self, anchored: bool) -> Option<Self> {
        (anchored || !self.admits(Hay::Bytes(b""))).then_some(self)
    }

    /// `false`: the haystack has no match of the exact text (under search, `re.match` and, on the fullmatch
    /// engines, `re.fullmatch` semantics alike).
    pub(crate) fn admits(&self, h: Hay<'_>) -> bool {
        match &self.0 {
            Screen::Regex(re) => re.is_match(h),
            Screen::Forward(f) => f.is_match(h),
        }
    }

    /// Whether the forward-only engine has its lazy DFA (`None`: the prefilter runs on `regex`).
    #[cfg(test)]
    pub(crate) fn has_lazy_dfa(&self) -> Option<bool> {
        match &self.0 {
            Screen::Regex(_) => None,
            Screen::Forward(f) => Some(f.dfa.is_some()),
        }
    }
}

/// The forward-only `is_match` engine of the tier-1 prefilter (ADR-108).
///
/// `regex`'s engine needs a forward *and* a reverse lazy DFA in one cache; with the prefilter's 256 KiB cap the
/// pair often does not fit (B12u's `\b\w{3,8}\b/\w+\.md$`: its relaxed text's reverse NFA has 12 366 states
/// against the forward one's 2 979, and the reverse DFA alone exceeds the cap), and it then runs the bounded
/// backtracker on every haystack, clearing its visited set each time. A yes/no answer
/// needs no match start, so this engine builds only the forward half, from the same syntax configuration as
/// [`T1`] (str: Unicode and UTF-8; bytes: neither; nest limit 2048; NFA size limit 64 MiB), without capture
/// states, and searches with `earliest`. When the DFA cannot be built at the capacity, gives up (cache
/// thrashing) or quits (a Unicode word boundary on a non-ASCII byte), the bounded backtracker answers on
/// haystacks of at most [`BACKTRACK_MAX_HAYSTACK`] bytes that fit its visited capacity, and the `PikeVM` on the
/// rest — the engines and the order of regex-automata `meta`'s `is_match_nofail`. Every engine answers
/// `is_match` exactly; only the cost differs.
#[derive(Debug)]
pub(crate) struct Forward {
    /// The lazy DFA (`None`: it could not be built at the cache capacity).
    dfa: Option<hybrid::dfa::DFA>,
    /// The bounded backtracker (`None`: it could not be built).
    backtrack: Option<backtrack::BoundedBacktracker>,
    /// The `PikeVM`: answers every haystack.
    pikevm: pikevm::PikeVM,
    /// The per-thread search caches.
    caches: Pool<ForwardCache>,
    /// A str pattern: bytes that are not UTF-8 never match.
    text: bool,
}

/// One thread's search state of a [`Forward`]; each part is created on its engine's first use, so a thread
/// whose searches the DFA answers never allocates the NFA engines' caches.
#[derive(Debug, Default)]
struct ForwardCache {
    dfa: Option<hybrid::dfa::Cache>,
    backtrack: Option<backtrack::Cache>,
    pikevm: Option<pikevm::Cache>,
}

impl Forward {
    /// Builds `text` (on a large stack when `depth` is deep); `None` when the text does not parse or its NFA
    /// exceeds the size limit.
    fn build(text: &str, bytes: bool, depth: u32, cache_capacity: usize) -> Option<Self> {
        on_stack(depth, || Self::build_here(text, bytes, cache_capacity)).flatten()
    }

    fn build_here(text: &str, bytes: bool, cache_capacity: usize) -> Option<Self> {
        let utf8 = !bytes;
        let syntax = syntax::Config::new()
            .unicode(utf8)
            .utf8(utf8)
            .nest_limit(NEST_LIMIT);
        let hir = syntax::parse_with(text, &syntax).ok()?;
        let nfa = thompson::Compiler::new()
            .configure(
                thompson::Config::new()
                    .utf8(utf8)
                    .nfa_size_limit(Some(SIZE_LIMIT))
                    .shrink(false)
                    .which_captures(WhichCaptures::None),
            )
            .build_from_hir(&hir)
            .ok()?;
        // A prefix-literal prefilter, as `meta` builds it: leftmost-first, and only for an unanchored search.
        let pre = if hir.properties().look_set_prefix().contains(Look::Start) {
            None
        } else {
            regex_automata::util::prefilter::Prefilter::from_hirs_prefix(
                regex_automata::MatchKind::LeftmostFirst,
                &[&hir],
            )
        };
        let dfa = hybrid::dfa::Builder::new()
            .configure(
                hybrid::dfa::Config::new()
                    .match_kind(regex_automata::MatchKind::LeftmostFirst)
                    .prefilter(pre.clone())
                    .specialize_start_states(pre.is_some())
                    .unicode_word_boundary(true)
                    .cache_capacity(cache_capacity)
                    .skip_cache_capacity_check(false)
                    .minimum_cache_clear_count(Some(3))
                    .minimum_bytes_per_state(Some(10)),
            )
            .build_from_nfa(nfa.clone())
            .ok();
        let backtrack = backtrack::Builder::new()
            .configure(backtrack::Config::new().prefilter(pre.clone()))
            .build_from_nfa(nfa.clone())
            .ok();
        let pikevm = pikevm::Builder::new()
            .configure(pikevm::Config::new().prefilter(pre))
            .build_from_nfa(nfa)
            .ok()?;
        Some(Self {
            dfa,
            backtrack,
            pikevm,
            caches: Pool::new(ForwardCache::default),
            text: utf8,
        })
    }

    /// `is_match` under search semantics (str engines never match bytes that are not UTF-8).
    fn is_match(&self, h: Hay<'_>) -> bool {
        let hay = if self.text {
            match h.text() {
                Some(t) => t.as_bytes(),
                None => return false,
            }
        } else {
            h.bytes()
        };
        let input = Input::new(hay).earliest(true);
        let mut cache = self.caches.get();
        if let Some(dfa) = &self.dfa {
            let c = cache.dfa.get_or_insert_with(|| dfa.create_cache());
            if let Ok(found) = dfa.try_search_fwd(c, &input) {
                #[cfg(test)]
                probe::count_forward(probe::Answer::Dfa);
                return found.is_some();
            }
            // Gave up or quit: the NFA engines answer.
        }
        self.is_match_nofail(&mut cache, &input)
    }

    /// The NFA engines' answer: the bounded backtracker when the haystack is short and fits its visited
    /// capacity, otherwise (or if it reports an error) the `PikeVM`, which never fails.
    fn is_match_nofail(&self, cache: &mut ForwardCache, input: &Input<'_>) -> bool {
        if let Some(bt) = &self.backtrack
            && input.haystack().len() <= BACKTRACK_MAX_HAYSTACK
            && input.get_span().len() <= bt.max_haystack_len()
        {
            let c = cache.backtrack.get_or_insert_with(|| bt.create_cache());
            if let Ok(found) = bt.try_is_match(c, input.clone()) {
                #[cfg(test)]
                probe::count_forward(probe::Answer::Backtrack);
                return found;
            }
        }
        #[cfg(test)]
        probe::count_forward(probe::Answer::PikeVm);
        let c = cache
            .pikevm
            .get_or_insert_with(|| self.pikevm.create_cache());
        self.pikevm.is_match(c, input.clone())
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
    /// span condition (ADR-098; not for a non-MULTILINE `$` alone).
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
    /// Relaxed prefilter text (ADR-098).
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
                    Prefilter::forward(
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
                Prefilter::regex(&wrap(r), texts.bytes, depth, texts.relaxed_anchored)
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
        static FORWARD: Cell<ForwardRuns> = const { Cell::new(ForwardRuns { dfa: 0, backtrack: 0, pikevm: 0 }) };
    }

    /// Which engine of [`super::Forward`] answered.
    #[derive(Copy, Clone, Debug)]
    pub(super) enum Answer {
        /// The lazy DFA.
        Dfa,
        /// The bounded backtracker.
        Backtrack,
        /// The `PikeVM`.
        PikeVm,
    }

    /// The answers of the forward-only prefilter engines on this thread so far, per engine.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub(crate) struct ForwardRuns {
        pub(crate) dfa: u64,
        pub(crate) backtrack: u64,
        pub(crate) pikevm: u64,
    }

    impl ForwardRuns {
        /// The answers since `earlier`.
        pub(crate) fn since(self, earlier: Self) -> Self {
            Self {
                dfa: self.dfa.saturating_sub(earlier.dfa),
                backtrack: self.backtrack.saturating_sub(earlier.backtrack),
                pikevm: self.pikevm.saturating_sub(earlier.pikevm),
            }
        }
    }

    /// One run of a backtracking engine.
    pub(super) fn count_exact_run() {
        EXACT_RUNS.with(|c| c.set(c.get().saturating_add(1)));
    }

    /// One answer of the forward-only prefilter engine.
    pub(super) fn count_forward(answer: Answer) {
        FORWARD.with(|c| {
            let mut runs = c.get();
            let slot = match answer {
                Answer::Dfa => &mut runs.dfa,
                Answer::Backtrack => &mut runs.backtrack,
                Answer::PikeVm => &mut runs.pikevm,
            };
            *slot = slot.saturating_add(1);
            c.set(runs);
        });
    }

    /// The forward-only prefilter engine's answers on this thread so far.
    pub(crate) fn forward_runs() -> ForwardRuns {
        FORWARD.with(Cell::get)
    }

    /// The backtracking-engine runs on this thread so far.
    pub(crate) fn exact_runs() -> u64 {
        EXACT_RUNS.with(Cell::get)
    }

    /// Whether the tier-1 exact fallback's prefilter is switched off on this thread.
    pub(super) fn prefilter_off() -> bool {
        PREFILTER_OFF.with(Cell::get)
    }

    /// Switches the tier-1 exact fallback's prefilter off (`true`: the pre-ADR-098 behaviour, the reference of
    /// the parity tests) or back on, for this thread.
    pub(crate) fn set_prefilter_off(off: bool) {
        PREFILTER_OFF.with(|c| c.set(off));
    }
}
