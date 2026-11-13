//! `validate`, `cfgv_is_valid` and the compiled [`PyRegex`] (01 Addendum §A.6.1, §A.6.9–§A.6.12,
//! ARCHITECTURE §3.3).
//!
//! Owner: WP-13.

use std::sync::OnceLock;

use memchr::memmem;

use crate::ast::{Ast, At, Features, Flags, Node};
use crate::engine::{Compiled, Hay, Op, Texts};
use crate::error::{MatchError, MatchKind, PyReError, PyWarning};
use crate::lower::{self, Variant};
use crate::messages;
use crate::options::{Mode, Options};
use crate::parse;

/// Expanded-size estimate above which the exact fallback is built eagerly (so that an engine limit is a
/// compile error, never a match-time one); typical patterns stay far below and build it lazily (§A.9).
const EAGER_EST: u64 = 20_000;

/// The engine budget (01 Addendum §A.6.13 D5): the `regex` compiler unrolls counted repeats, so its time and
/// memory grow with the expansion ([`crate::ast::Info::est`], ~µs of compile time). Above ~0.1 s of estimated
/// work the pattern is `Unsupported` before any engine is built — `a{40000}`, `[0-9a-f]{5000}` and `\w{90}`
/// compile, `a{100000}`, `\w{1000}` and nested counts like `(?:a{999}){999}` do not (CPython compiles them
/// without unrolling). Anchors are charged by their exact look-around forms (a Unicode `\b`/`\B` ~200, a
/// non-MULTILINE `$` 50), so runs of ~500 word boundaries or ~2 000 `$` are the limit. The corpus maximum is a count
/// of 111 on single characters.
const EXPANSION_BUDGET: u64 = 100_000;

/// Parse + post-parse checks only; never builds an engine (validate-config, config load; ARCHITECTURE §3.3
/// item 7). Returns the `FutureWarning`s CPython 3.12's `re.compile(src)` would emit.
pub fn validate(src: &str, mode: Mode) -> Result<Vec<PyWarning>, PyReError> {
    parse::parse(src, mode, Flags::empty()).map(|p| p.warnings)
}

/// For cfgv 0.3 `CheckFn::check_regex_with` (bool callback, ADR-003): `validate(src, Mode::Str).is_ok()`. Every
/// rejection — including Overflow/Value/Recursion — becomes cfgv's "is not a valid python regex" leaf (D-CRASH-04);
/// `check_regex_try_with` (which could reproduce the exit-3 crash) is deliberately unused (§1.5).
pub fn cfgv_is_valid(src: &str) -> bool {
    validate(src, Mode::Str).is_ok()
}

/// A compiled Python pattern (`re.compile(src, flags)`): the cheap match kinds of ARCHITECTURE §3.3 item 4, or
/// the two-tier engines of 01 Addendum §A.6.10 (exact fallbacks and the `re.match`/`re.fullmatch` variants
/// are built on first use). `Send + Sync`; shared as `Arc<PyRegex>` (ADR-081).
#[derive(Debug)]
pub struct PyRegex {
    src: Box<str>,
    opts: Options,
    kind: MatchKind,
    warnings: Box<[PyWarning]>,
    imp: Imp,
}

/// The matcher behind a [`PyRegex`].
#[derive(Debug)]
enum Imp {
    /// `''`.
    All,
    /// `^$`.
    EmptyOrNewline,
    /// `^lit` / `\Alit`.
    Prefix(Box<[u8]>),
    /// `lit$` (`dollar`) or `lit\Z`.
    Suffix { lit: Box<[u8]>, dollar: bool },
    /// `lit`.
    Contains(Box<memmem::Finder<'static>>),
    /// The engines.
    Engines(Box<Engines>),
}

/// Engines and the texts the lazy `re.match` / `re.fullmatch` variants are built from.
#[derive(Debug)]
struct Engines {
    texts: Texts,
    /// `re.search`, and `re.match` unless `prefix` exists.
    search: Compiled,
    /// `re.match` of a pattern whose search engines carry the charset-prefix rule ([`lower::Quirk`]), built on
    /// first use from `texts`.
    prefix: Option<OnceLock<Option<Compiled>>>,
    /// `re.fullmatch` (`\A(?:P)\z`), built on first use.
    full: OnceLock<Option<Compiled>>,
    eager: bool,
}

impl Engines {
    fn is_match(&self, h: Hay<'_>, op: Op) -> Result<bool, MatchError> {
        if op == Op::Prefix
            && let Some(cell) = &self.prefix
        {
            let prefix = cell.get_or_init(|| Compiled::build(&self.texts, false, self.eager).ok());
            let Some(prefix) = prefix else {
                return Err(MatchError::Backtrack);
            };
            return prefix.is_match(h, Op::Prefix);
        }
        if op != Op::Full {
            return self.search.is_match(h, op);
        }
        let full = self
            .full
            .get_or_init(|| Compiled::build(&self.texts, true, self.eager).ok());
        // Unreachable in practice: the search engines of the same text built, and the expansion budget bounds
        // the extra `\A(?:…)\z`.
        let Some(full) = full else {
            return Err(MatchError::Backtrack);
        };
        full.is_match(h, Op::Search)
    }
}

impl PyRegex {
    /// `re.compile(src, flags)` with `flags` from `opts` (pygrep: `re.IGNORECASE`, `re.MULTILINE | re.DOTALL`);
    /// the search engines are built here, the exact fallbacks and anchored variants on first use.
    pub fn compile(src: &str, opts: Options) -> Result<Self, PyReError> {
        let mut init = Flags::empty();
        if opts.ignore_case {
            init = init.or(Flags::I);
        }
        if opts.multiline_dotall {
            init = init.or(Flags::M).or(Flags::S);
        }
        let parsed = parse::parse(src, opts.mode, init)?;
        let warnings = parsed.warnings.into_boxed_slice();
        let ast = parsed.ast;
        let (kind, imp) = match fast_kind(&ast) {
            Some(fast) => fast,
            None => engines(&ast, opts)?,
        };
        Ok(Self {
            src: src.into(),
            opts,
            kind,
            warnings,
            imp,
        })
    }

    /// The pattern source.
    pub fn source(&self) -> &str {
        &self.src
    }

    /// The selected match kind.
    pub fn kind(&self) -> MatchKind {
        self.kind
    }

    /// `FutureWarning`s raised by the compile, in order (01 Addendum §A.4).
    pub fn warnings(&self) -> &[PyWarning] {
        &self.warnings
    }

    /// `re.search(p, h) is not None` (`pre_commit/commands/run.py:60-70` for `files`/`exclude`).
    pub fn search(&self, h: &str) -> Result<bool, MatchError> {
        self.is_match(Hay::Str(h), Op::Search)
    }

    /// `re.match(p, h) is not None`: a match starting at position 0 (`\A(?:P)`).
    pub fn match_prefix(&self, h: &str) -> Result<bool, MatchError> {
        self.is_match(Hay::Str(h), Op::Prefix)
    }

    /// `re.fullmatch(p, h) is not None`: a match spanning `h` (`\A(?:P)\z`).
    pub fn fullmatch(&self, h: &str) -> Result<bool, MatchError> {
        self.is_match(Hay::Str(h), Op::Full)
    }

    /// `re.search(p, h)` over bytes with the match span in bytes (pygrep, `pre_commit/languages/pygrep.py`).
    /// A str pattern never matches bytes that are not UTF-8.
    pub fn search_bytes(&self, h: &[u8]) -> Result<Option<(usize, usize)>, MatchError> {
        let hay = Hay::Bytes(h);
        if self.opts.mode == Mode::Str
            && !matches!(self.imp, Imp::Engines(_))
            && hay.text().is_none()
        {
            return Ok(None);
        }
        match &self.imp {
            Imp::All => Ok(Some((0, 0))),
            Imp::EmptyOrNewline => Ok((h.is_empty() || h == b"\n").then_some((0, 0))),
            Imp::Prefix(lit) => Ok(h.starts_with(lit).then_some((0, lit.len()))),
            Imp::Suffix { lit, dollar } => Ok(suffix_span(h, lit, *dollar)),
            Imp::Contains(f) => Ok(f.find(h).map(|i| (i, i.saturating_add(f.needle().len())))),
            Imp::Engines(e) => e.search.find(hay),
        }
    }

    fn is_match(&self, h: Hay<'_>, w: Op) -> Result<bool, MatchError> {
        let b = h.bytes();
        Ok(match (&self.imp, w) {
            (Imp::All, Op::Search | Op::Prefix) => true,
            (Imp::All | Imp::EmptyOrNewline, Op::Full) => b.is_empty(),
            (Imp::EmptyOrNewline, _) => b.is_empty() || b == b"\n",
            (Imp::Prefix(lit), Op::Search | Op::Prefix) => b.starts_with(lit),
            (Imp::Suffix { lit, dollar }, Op::Search) => suffix_span(b, lit, *dollar).is_some(),
            (Imp::Suffix { lit, dollar }, Op::Prefix) => {
                b == lit.as_ref() || (*dollar && is_lit_nl(b, lit))
            }
            (Imp::Contains(f), Op::Search) => f.find(b).is_some(),
            (Imp::Contains(f), Op::Prefix) => b.starts_with(f.needle()),
            (Imp::Prefix(lit) | Imp::Suffix { lit, .. }, Op::Full) => b == lit.as_ref(),
            (Imp::Contains(f), Op::Full) => b == f.needle(),
            (Imp::Engines(e), op) => return e.is_match(h, op),
        })
    }
}

/// `h == lit + "\n"`.
fn is_lit_nl(h: &[u8], lit: &[u8]) -> bool {
    h.split_last()
        .is_some_and(|(last, head)| *last == b'\n' && head == lit)
}

/// The leftmost match of `lit$` (Python `$`: the end, or before a final `\n`) or `lit\Z`.
fn suffix_span(h: &[u8], lit: &[u8], dollar: bool) -> Option<(usize, usize)> {
    let n = lit.len();
    if dollar && h.last() == Some(&b'\n') {
        let end = h.len().saturating_sub(1);
        if h.get(..end).is_some_and(|head| head.ends_with(lit)) {
            return Some((end.saturating_sub(n), end));
        }
    }
    h.ends_with(lit)
        .then(|| (h.len().saturating_sub(n), h.len()))
}

/// The cheap match kinds (ARCHITECTURE §3.3 item 4): the AST is `''`, `^$`, or a plain literal with at most
/// one non-MULTILINE anchor, under no IGNORECASE.
fn fast_kind(ast: &Ast) -> Option<(MatchKind, Imp)> {
    let items = ast.seq(ast.root);
    if items.is_empty() {
        return Some((MatchKind::All, Imp::All));
    }
    let multiline = ast.flags.has(Flags::M);
    let node = |i: usize| items.get(i).and_then(|&id| ast.node(id));
    let is_begin = |n: Option<&Node>| match n {
        Some(Node::At(At::Beginning)) => !multiline,
        Some(Node::At(At::BeginningString)) => true,
        _ => false,
    };
    let end = |n: Option<&Node>| match n {
        Some(Node::At(At::End)) if !multiline => Some(true),
        Some(Node::At(At::EndString)) => Some(false),
        _ => None,
    };
    if items.len() == 2 && is_begin(node(0)) && end(node(1)) == Some(true) {
        return Some((MatchKind::EmptyOrNewline, Imp::EmptyOrNewline));
    }
    let head = is_begin(node(0));
    let tail = end(node(items.len().saturating_sub(1)));
    if head && tail.is_some() {
        return None;
    }
    let literals =
        items.get(usize::from(head)..items.len().saturating_sub(usize::from(tail.is_some())))?;
    if ast.flags.has(Flags::I) && !literals.is_empty() {
        return None;
    }
    let mut lit: Vec<u8> = Vec::new();
    for &id in literals {
        let Some(Node::Lit(c)) = ast.node(id) else {
            return None;
        };
        if ast.bytes {
            lit.push(u8::try_from(*c).ok()?);
        } else {
            let ch = char::from_u32(*c)?;
            let mut buf = [0_u8; 4];
            lit.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    let lit = lit.into_boxed_slice();
    Some(match (head, tail) {
        (true, _) => (MatchKind::Prefix, Imp::Prefix(lit)),
        (false, Some(dollar)) => (MatchKind::Suffix, Imp::Suffix { lit, dollar }),
        (false, None) => (
            MatchKind::Contains,
            Imp::Contains(Box::new(memmem::Finder::new(&lit).into_owned())),
        ),
    })
}

/// Lowers and builds the engines of a pattern that is not a cheap kind.
fn engines(ast: &Ast, opts: Options) -> Result<(MatchKind, Imp), PyReError> {
    let info = ast.root_info();
    if info.est > EXPANSION_BUDGET {
        return Err(PyReError::Unsupported(messages::OVER_BUDGET.into()));
    }
    let tier2 = info.feat.has(Features::TIER2);
    let texts = texts(ast, opts, tier2, None)?;
    let quirk = lower::search_quirk(ast);
    let search_texts = match &quirk {
        Some(q) => Some(self::texts(ast, opts, tier2, Some(q))?),
        None => None,
    };
    let eager = info.est > EAGER_EST;
    let search = Compiled::build(search_texts.as_ref().unwrap_or(&texts), false, eager)
        .map_err(|e| PyReError::Unsupported(e.into_boxed_str()))?;
    let kind = if tier2 {
        MatchKind::Tier2
    } else {
        MatchKind::Tier1
    };
    let engines = Engines {
        texts,
        search,
        prefix: quirk.map(|_| OnceLock::new()),
        full: OnceLock::new(),
        eager,
    };
    Ok((kind, Imp::Engines(Box::new(engines))))
}

/// The lowered texts of `ast` (`quirk`: the `re.search` variant of [`lower::Quirk`]).
fn texts(
    ast: &Ast,
    opts: Options,
    tier2: bool,
    quirk: Option<&lower::Quirk>,
) -> Result<Texts, PyReError> {
    let bytes = ast.bytes;
    let texts = if tier2 {
        let exact = lower::lower_with(ast, Variant::Exact, opts.haystack, quirk);
        let relaxed = lower::lower(ast, Variant::Relaxed, opts.haystack);
        Texts {
            main: exact.text.into_boxed_str(),
            exact: None,
            relaxed: Some(relaxed.text.into_boxed_str()),
            tier2,
            cond: lower::Cond::default(),
            depth: exact.depth.max(relaxed.depth),
            bytes,
            relaxed_anchored: relaxed.anchored,
        }
    } else {
        let mut fast = lower::lower_with(ast, Variant::Fast, opts.haystack, quirk);
        let mut exact = (!fast.cond.is_none())
            .then(|| lower::lower_with(ast, Variant::Exact, opts.haystack, quirk));
        if let Some(e) = &exact
            && e.depth >= crate::engine::FANCY_MAX_DEPTH
        {
            let only_span = lower::Cond {
                span: false,
                ..fast.cond
            }
            .is_none();
            if !only_span {
                return Err(PyReError::Unsupported(messages::TOO_DEEP_FOR_TIER2.into()));
            }
            // Only the match span of a nullable loop needed the exact form: keep `regex`'s span (D-MISC-06)
            // rather than rejecting a pattern whose matches are exact.
            fast.cond.span = false;
            exact = None;
        }
        // ADR-097: the exact fallback of a word-boundary, `\B` or span condition runs behind the relaxed text
        // (without the search quirk, which only narrows the start positions), the same prefilter as tier 2. A
        // `$`-only condition keeps none: its exact text has no word assertion to make the backtracking engine
        // slow, and the haystacks it serves end with `\n`.
        let c = fast.cond;
        let relaxed = (exact.is_some() && (c.ascii || c.nonempty || c.span))
            .then(|| lower::lower(ast, Variant::Relaxed, opts.haystack));
        let depth = [
            Some(fast.depth),
            exact.as_ref().map(|e| e.depth),
            relaxed.as_ref().map(|r| r.depth),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(fast.depth);
        Texts {
            main: fast.text.into_boxed_str(),
            exact: exact.map(|e| e.text.into_boxed_str()),
            relaxed_anchored: relaxed.as_ref().is_some_and(|r| r.anchored),
            relaxed: relaxed.map(|r| r.text.into_boxed_str()),
            tier2,
            cond: fast.cond,
            depth,
            bytes,
        }
    };
    if tier2 && texts.depth >= crate::engine::FANCY_MAX_DEPTH {
        return Err(PyReError::Unsupported(messages::TOO_DEEP_FOR_TIER2.into()));
    }
    Ok(texts)
}

#[cfg(test)]
pub(crate) mod testing {
    //! Internal views for the crate's own tests.
    use super::*;

    /// All lowered texts and facts of `src` (engines only).
    pub(crate) fn full_texts(src: &str, opts: Options) -> Option<Texts> {
        let re = PyRegex::compile(src, opts).ok()?;
        match re.imp {
            Imp::Engines(e) => Some(e.texts),
            _ => None,
        }
    }

    /// `PyRegex::compile` without the cheap match kinds (every pattern goes to the engines).
    pub(crate) fn compile_engines(src: &str, opts: Options) -> Result<PyRegex, PyReError> {
        let mut init = Flags::empty();
        if opts.ignore_case {
            init = init.or(Flags::I);
        }
        if opts.multiline_dotall {
            init = init.or(Flags::M).or(Flags::S);
        }
        let parsed = parse::parse(src, opts.mode, init)?;
        let (kind, imp) = engines(&parsed.ast, opts)?;
        Ok(PyRegex {
            src: src.into(),
            opts,
            kind,
            warnings: parsed.warnings.into_boxed_slice(),
            imp,
        })
    }
}
