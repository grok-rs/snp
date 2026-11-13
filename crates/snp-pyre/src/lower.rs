//! Lowering of the parsed pattern to engine syntax (01 Addendum §A.6.4–§A.6.8, §A.6.11).
//!
//! One emitter, three variants:
//! * [`Variant::Fast`]: tier-1 (`regex`) text with the cheaper anchor forms of §A.6.5 and the condition bits
//!   that tell when a haystack needs the exact variant;
//! * [`Variant::Exact`]: the canonical forms (`$` → `(?=\n?\z)`, look-around `\b`/`\B`) for `fancy-regex`;
//! * [`Variant::Relaxed`]: the tier-1 prefilter of a tier-2 pattern and of a tier-1 exact fallback (ADR-097)
//!   (look-around → nothing, back-reference → `(?s:.*)`, conditional → alternation, atomic/possessive → plain,
//!   `$` → `(?m:$)`, str `\b`/`\B` → nothing), whose language is a superset of the pattern's under search
//!   semantics, at every span.
//!
//! Every literal except `[0-9A-Za-z]` is hex-escaped (str `\x{H}`, bytes `\xHH`); no inline flag is emitted
//! except `(?s:.)`, `(?m:^)`, `(?m:$)` and `(?i:\N)`; capturing groups keep Python's numbering. The walk is
//! iterative (explicit work stack).
//!
//! Owner: WP-13.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::ast::{Ast, At, Cat, Features, Flags, MAXREPEAT, Node, NodeId, RepKind, SeqId, SetItem};
use crate::fold::RangeSet;
use crate::options::Haystack;

/// Which text to produce.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Tier 1 with haystack-dependent fast forms.
    Fast,
    /// Canonical forms for the backtracking engine.
    Exact,
    /// The tier-1 prefilter of a tier-2 pattern, and of the exact fallback of a tier-1 pattern with a `\b`/`\B`
    /// or span condition (ADR-097).
    Relaxed,
}

/// When a haystack must use the exact variant instead of the fast one (§A.6.5).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent condition bits, tested one by one"
)]
pub(crate) struct Cond {
    /// A non-MULTILINE `$` became `\z`: haystacks ending with `\n` need the exact form.
    pub(crate) nl: bool,
    /// A str `\b`/`\B` became Rust's: non-ASCII haystacks need the exact form.
    pub(crate) ascii: bool,
    /// A `\B` became Rust's: the empty haystack needs the exact form (Python's `\B` is false there).
    pub(crate) nonempty: bool,
    /// A greedy loop over a nullable body: `regex` keeps iterating after an empty iteration where CPython's
    /// `MAX_UNTIL` stops (`_sre` `last_ptr`), which changes the match *end* but never whether a match exists, so
    /// only match spans (`search_bytes`) need the exact form.
    pub(crate) span: bool,
}

impl Cond {
    /// No condition: the fast form is exact for every haystack.
    pub(crate) const fn is_none(self) -> bool {
        !self.nl && !self.ascii && !self.nonempty && !self.span
    }

    /// `use_exact` of §A.6.5 for haystack `h`.
    pub(crate) fn needs_exact(self, h: &[u8]) -> bool {
        (self.nl && h.last() == Some(&b'\n'))
            || (self.nonempty && h.is_empty())
            || (self.ascii && !h.is_ascii())
    }

    /// `use_exact` for a match span: [`Cond::needs_exact`] or a nullable greedy loop.
    pub(crate) fn needs_exact_span(self, h: &[u8]) -> bool {
        self.span || self.needs_exact(h)
    }
}

/// An emitted text and its facts.
#[derive(Clone, Debug, Default)]
pub(crate) struct Lowered {
    /// The engine syntax.
    pub(crate) text: String,
    /// Condition bits (fast variant only).
    pub(crate) cond: Cond,
    /// The deepest group nesting of `text` (fancy-regex accepts at most 63).
    pub(crate) depth: u32,
    /// The text contains an anchor or a word boundary (decides whether a relaxed prefilter can be skipped).
    pub(crate) anchored: bool,
}

/// The never-matching class (§A.6.4).
const NEVER_STR: &str = "[^\\x{0}-\\x{10FFFF}]";
const NEVER_BYTES: &str = "[^\\x00-\\xFF]";

/// One pending piece of output.
#[derive(Debug)]
enum Work {
    Seq(SeqId, Flags),
    Node(NodeId, Flags),
    Text(&'static str),
    Owned(String),
    /// Enter / leave a copy whose capture groups are emitted as `(?:…)`.
    Plain(bool),
}

/// The emitter.
#[derive(Debug)]
struct Emitter<'a> {
    ast: &'a Ast,
    variant: Variant,
    haystack: Haystack,
    out: String,
    cond: Cond,
    anchored: bool,
    numbering: Numbering,
    /// Inside a copy emitted without capture groups (see [`Emitter::repeat`]).
    plain: u32,
    /// The `re.search` charset-prefix rule, when it applies (see [`search_quirk`]).
    quirk: Option<&'a Quirk>,
}

/// CPython 3.12's search-only charset prefix (`re/_compiler.py` `_compile_info`): when a pattern of nonzero
/// minimum width starts (through groups) with a set, `_get_charset_prefix` returns that set, but
/// `_compile_charset` then compiles it with the *top-level* flags, so for `(?a:\W)` the prefix is the Unicode
/// `\W`, and `re.search` only tries the positions whose character is in it (`_sre` `SRE(search)`), while
/// `re.match`/`re.fullmatch` use the ASCII set. The search texts intersect the first set with the prefix set.
#[derive(Debug, Clone)]
pub(crate) struct Quirk {
    /// The nodes (`Lit`/`In`) that consume the first character.
    nodes: Vec<NodeId>,
    /// The prefix set, rendered with Unicode categories.
    negate: bool,
    items: Vec<SetItem>,
}

/// [`Quirk`] of `ast`, when its search differs from its match (str patterns whose leading set lies in a scoped
/// `(?a:…)` group and contains a category).
pub(crate) fn search_quirk(ast: &Ast) -> Option<Quirk> {
    if ast.bytes || !ast.flags.has(Flags::U) || ast.root_info().lo == 0 {
        return None;
    }
    let mut fl = ast.flags;
    let mut seq = ast.root;
    // `while True: … if op is not SUBPATTERN: break` (`_get_charset_prefix`).
    let (first, node) = loop {
        let &first = ast.seq(seq).first()?;
        match ast.node(first)? {
            Node::Group { add, del, body, .. } => {
                fl = fl.combine(*add, *del);
                seq = *body;
            }
            node => break (first, node),
        }
    };
    if !fl.has(Flags::A) {
        return None;
    }
    // `_get_iscased` under the combined (ASCII) flags: ASCII letters are cased.
    let ignore = fl.has(Flags::I);
    let cased = |c: u32| ignore && char::from_u32(c).is_some_and(|ch| ch.is_ascii_alphabetic());
    let set_ok = |items: &[SetItem]| {
        items.iter().all(|it| match *it {
            SetItem::Lit(c) => !cased(c),
            SetItem::Range(lo, hi) => !(ignore && (hi > 0xFFFF || (lo..=hi).any(cased))),
            SetItem::Cat(_) => true,
        })
    };
    let has_cat = |items: &[SetItem]| items.iter().any(|it| matches!(it, SetItem::Cat(_)));
    match node {
        Node::In { negate, items } => (set_ok(items) && has_cat(items)).then(|| Quirk {
            nodes: vec![first],
            negate: *negate,
            items: items.to_vec(),
        }),
        Node::Branch(alts) => {
            // `_parse_sub` first moves a first item shared by every alternative out of the branch.
            let firsts: Option<Vec<NodeId>> =
                alts.iter().map(|&a| ast.seq(a).first().copied()).collect();
            if let Some(firsts) = firsts
                && let Some(Node::In { negate, items }) = firsts.first().and_then(|&n| ast.node(n))
                && firsts.iter().all(|&n| {
                    matches!(ast.node(n), Some(Node::In { negate: g, items: it }) if g == negate && it == items)
                })
            {
                return (set_ok(items) && has_cat(items)).then(|| Quirk {
                    nodes: firsts.clone(),
                    negate: *negate,
                    items: items.to_vec(),
                });
            }
            // `_parse_sub` turns a branch of single literals and non-negated sets into one set.
            let mut nodes = Vec::new();
            let mut items = Vec::new();
            for &a in alts {
                let [only] = ast.seq(a) else { return None };
                match ast.node(*only)? {
                    Node::Lit(c) => items.push(SetItem::Lit(*c)),
                    Node::In {
                        negate: false,
                        items: it,
                    } => items.extend_from_slice(it),
                    _ => return None,
                }
                nodes.push(*only);
            }
            (set_ok(&items) && has_cat(&items)).then_some(Quirk {
                nodes,
                negate: false,
                items,
            })
        }
        _ => None,
    }
}

/// Capture numbering of the exact text. CPython's `GROUPREF_EXISTS` (`_sre` `SRE_OP_GROUPREF_EXISTS`) treats
/// a group as matched only once it has *closed* (both marks set), while fancy-regex tests the start slot; so
/// every group a conditional refers to is followed by an empty "completed" group `()`, which the conditional
/// tests instead. Python's group numbers shift by the auxiliary groups opened before them.
#[derive(Debug, Default)]
struct Numbering {
    /// Python group → emitted group (only when auxiliary groups exist).
    emitted: HashMap<u32, u32>,
    /// Python group → its emitted "completed" group.
    aux: HashMap<u32, u32>,
}

impl Numbering {
    fn of(&self, gid: u32) -> u32 {
        self.emitted.get(&gid).copied().unwrap_or(gid)
    }

    fn completed(&self, gid: u32) -> u32 {
        self.aux.get(&gid).copied().unwrap_or_else(|| self.of(gid))
    }

    /// Walks the tree in emission (pre-)order.
    fn new(ast: &Ast) -> Self {
        enum Ev {
            Seq(SeqId),
            Node(NodeId),
            Exit(u32),
        }
        let referenced: HashSet<u32> = ast
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::GroupRefExists { gid, .. } => Some(*gid),
                _ => None,
            })
            .collect();
        if referenced.is_empty() {
            return Self::default();
        }
        let mut out = Self::default();
        let mut counter = 0_u32;
        let mut stack = vec![Ev::Seq(ast.root)];
        while let Some(ev) = stack.pop() {
            match ev {
                Ev::Seq(s) => stack.extend(ast.seq(s).iter().rev().map(|&n| Ev::Node(n))),
                Ev::Exit(g) => {
                    if referenced.contains(&g) {
                        counter = counter.saturating_add(1);
                        out.aux.insert(g, counter);
                    }
                }
                Ev::Node(n) => match ast.node(n) {
                    Some(Node::Group {
                        gid: Some(g), body, ..
                    }) => {
                        counter = counter.saturating_add(1);
                        out.emitted.insert(*g, counter);
                        stack.push(Ev::Exit(*g));
                        stack.push(Ev::Seq(*body));
                    }
                    Some(
                        Node::Group { body, .. }
                        | Node::Repeat { body, .. }
                        | Node::Assert { body, .. }
                        | Node::Atomic(body),
                    ) => stack.push(Ev::Seq(*body)),
                    Some(Node::Branch(alts)) => {
                        stack.extend(alts.iter().rev().map(|&a| Ev::Seq(a)));
                    }
                    Some(Node::GroupRefExists { yes, no, .. }) => {
                        if let Some(no) = no {
                            stack.push(Ev::Seq(*no));
                        }
                        stack.push(Ev::Seq(*yes));
                    }
                    _ => {}
                },
            }
        }
        out
    }
}

/// Lowers `ast` (its global flags already include the compile options).
pub(crate) fn lower(ast: &Ast, variant: Variant, haystack: Haystack) -> Lowered {
    lower_with(ast, variant, haystack, None)
}

/// [`lower`] for the `re.search` engines of a pattern with a [`Quirk`].
pub(crate) fn lower_with(
    ast: &Ast,
    variant: Variant,
    haystack: Haystack,
    quirk: Option<&Quirk>,
) -> Lowered {
    let numbering = if variant == Variant::Exact {
        Numbering::new(ast)
    } else {
        Numbering::default()
    };
    let mut e = Emitter {
        ast,
        variant,
        haystack,
        out: String::new(),
        cond: Cond::default(),
        anchored: false,
        numbering,
        plain: 0,
        quirk,
    };
    let mut hint = 0_usize;
    for s in &ast.seqs {
        hint = hint.saturating_add(s.items.len());
    }
    e.out
        .try_reserve(hint.saturating_mul(4))
        .unwrap_or_default();
    let mut stack = vec![Work::Seq(ast.root, ast.flags)];
    while let Some(w) = stack.pop() {
        match w {
            Work::Text(t) => e.out.push_str(t),
            Work::Owned(t) => e.out.push_str(&t),
            Work::Plain(true) => e.plain = e.plain.saturating_add(1),
            Work::Plain(false) => e.plain = e.plain.saturating_sub(1),
            Work::Seq(id, fl) => {
                for &n in ast.seq(id).iter().rev() {
                    stack.push(Work::Node(n, fl));
                }
            }
            Work::Node(id, fl) => e.node(id, fl, &mut stack),
        }
    }
    let depth = nesting_depth(&e.out);
    Lowered {
        text: e.out,
        cond: e.cond,
        depth,
        anchored: e.anchored,
    }
}

/// The deepest `(` nesting of an emitted text (raw parentheses only appear as group syntax: every literal
/// parenthesis is hex-escaped).
pub(crate) fn nesting_depth(text: &str) -> u32 {
    let mut depth = 0_u32;
    let mut max = 0_u32;
    let mut escaped = false;
    for b in text.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'(' => {
                depth = depth.saturating_add(1);
                max = max.max(depth);
            }
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

impl Emitter<'_> {
    fn bytes(&self) -> bool {
        self.ast.bytes
    }

    /// ASCII semantics for categories and folding: bytes patterns, `(?a)`, `(?L)` (§A.6.3).
    fn ascii(&self, fl: Flags) -> bool {
        self.bytes() || fl.has(Flags::A) || fl.has(Flags::L)
    }

    fn never(&self) -> &'static str {
        if self.bytes() { NEVER_BYTES } else { NEVER_STR }
    }

    /// Pushes the pieces of `parts` so that they are emitted in order.
    fn push_seq_of(stack: &mut Vec<Work>, parts: Vec<Work>) {
        stack.extend(parts.into_iter().rev());
    }

    fn node(&mut self, id: NodeId, fl: Flags, stack: &mut Vec<Work>) {
        let Some(node) = self.ast.node(id) else {
            return;
        };
        if let Some(q) = self.quirk
            && q.nodes.contains(&id)
        {
            let own = match node {
                Node::Lit(c) => self.class(false, &[SetItem::Lit(*c)], fl),
                Node::In { negate, items } => self.class(*negate, items, fl),
                _ => String::new(),
            };
            let prefix = self.class(q.negate, &q.items, Flags::empty());
            write!(self.out, "[{own}&&{prefix}]").unwrap_or_default();
            return;
        }
        match node {
            Node::Lit(c) => {
                let t = self.literal(*c, fl);
                self.out.push_str(&t);
            }
            Node::NotLit(c) => {
                let t = self.class(true, &[SetItem::Lit(*c)], fl);
                self.out.push_str(&t);
            }
            Node::Any => self
                .out
                .push_str(if fl.has(Flags::S) { "(?s:.)" } else { "." }),
            Node::In { negate, items } => {
                let t = self.class(*negate, items, fl);
                self.out.push_str(&t);
            }
            Node::At(at) => {
                let t = self.anchor(*at, fl);
                self.out.push_str(t);
            }
            Node::Branch(alts) => {
                let mut parts = vec![Work::Text("(?:")];
                for (i, &a) in alts.iter().enumerate() {
                    if i > 0 {
                        parts.push(Work::Text("|"));
                    }
                    parts.push(Work::Seq(a, fl));
                }
                parts.push(Work::Text(")"));
                Self::push_seq_of(stack, parts);
            }
            Node::Group {
                gid,
                add,
                del,
                body,
            } => {
                let inner = fl.combine(*add, *del);
                let exact = self.variant == Variant::Exact;
                let exact = exact && self.plain == 0;
                let open = if gid.is_some() && exact { "(" } else { "(?:" };
                let close = match gid {
                    Some(g) if exact && self.numbering.aux.contains_key(g) => ")()",
                    _ => ")",
                };
                Self::push_seq_of(
                    stack,
                    vec![Work::Text(open), Work::Seq(*body, inner), Work::Text(close)],
                );
            }
            Node::Repeat {
                kind,
                min,
                max,
                body,
            } => self.repeat(*kind, *min, *max, *body, fl, stack),
            Node::GroupRef(g) => {
                let g = self.numbering.of(*g);
                if self.variant == Variant::Relaxed {
                    self.out.push_str("(?s:.*)");
                } else if fl.has(Flags::I) {
                    write!(self.out, "(?i:\\{g})").unwrap_or_default();
                } else {
                    write!(self.out, "(?:\\{g})").unwrap_or_default();
                }
            }
            Node::GroupRefExists { gid, yes, no } => {
                let mut parts = if self.variant == Variant::Relaxed {
                    vec![Work::Text("(?:"), Work::Seq(*yes, fl), Work::Text("|")]
                } else {
                    // `(?(g)Y|N)` as `(?:\cY|(?!\c)N)` over the empty "completed" group `c` of g: a back-reference
                    // to an empty capture succeeds exactly when the capture is set. fancy-regex's own conditional
                    // is avoided: it tests group *starts*, ignores enclosing atomic groups and possessive repeats
                    // when backtracking into its branches, and turns `(?(1)|)` into an assertion.
                    let c = self.numbering.completed(*gid);
                    vec![
                        Work::Owned(format!("(?:(?:\\{c})")),
                        Work::Seq(*yes, fl),
                        Work::Owned(format!("|(?!\\{c})")),
                    ]
                };
                if let Some(n) = no {
                    parts.push(Work::Seq(*n, fl));
                }
                parts.push(Work::Text(")"));
                Self::push_seq_of(stack, parts);
            }
            Node::Assert {
                behind,
                negate,
                body,
            } => {
                if self.variant == Variant::Relaxed {
                    return;
                }
                let open = match (behind, negate) {
                    (false, false) => "(?=",
                    (false, true) => "(?!",
                    (true, false) => "(?<=",
                    (true, true) => "(?<!",
                };
                Self::push_seq_of(
                    stack,
                    vec![Work::Text(open), Work::Seq(*body, fl), Work::Text(")")],
                );
            }
            Node::Atomic(body) => {
                let open = if self.variant == Variant::Relaxed {
                    "(?:"
                } else {
                    "(?>"
                };
                Self::push_seq_of(
                    stack,
                    vec![Work::Text(open), Work::Seq(*body, fl), Work::Text(")")],
                );
            }
        }
    }

    /// §A.6.8, plus three rewrites of the exact (fancy-regex) text:
    ///
    /// * a greedy loop over a nullable body runs each iteration as `(?:(?=)X)`: the empty look-ahead makes the
    ///   loop "hard", so fancy-regex runs it on its VM, whose `RepeatEpsilonGr` leaves the loop after an empty
    ///   iteration exactly like CPython's `MAX_UNTIL` (`_sre` `last_ptr`), instead of delegating it to `regex`,
    ///   which drops the empty path and keeps iterating (`(?:x*|y)+` on `xyx` is `(0, 1)` in CPython);
    /// * an unbounded `*` over a body with a capture group gets an extra never-matching alternative
    ///   (`(?:X|NEVER)*`), and a lazy repeat that may run zero times is wrapped as `(?:X??|NEVER)`: fancy-regex
    ///   0.19's tree optimizer rewrites `(X+?)*` to `(X+?)?` and `a*b??a*` to `a*(?:ba*)?`, which changes the
    ///   match span when a lazy quantifier is involved; the extra alternative hides the shapes it matches.
    fn repeat(
        &mut self,
        kind: RepKind,
        min: u32,
        max: u32,
        body: SeqId,
        fl: Flags,
        stack: &mut Vec<Work>,
    ) {
        let zero_width = self.ast.seq_facts(body).hi == 0;
        if max == 0 {
            // `X{0}` never runs X; keep X's groups (Python numbering) behind a never-matching class.
            let parts = vec![
                Work::Text("(?:"),
                Work::Text(self.never()),
                Work::Text("(?:"),
                Work::Seq(body, fl),
                Work::Text(")|)"),
            ];
            Self::push_seq_of(stack, parts);
            return;
        }
        // `SRE_OP_POSSESSIVE_REPEAT` runs every iteration as a sub-match that returns its first success, then
        // never gives an iteration back: `(?>(?>X){m,n})`, not fancy-regex's `X{m,n}+` = `(?>X{m,n})`.
        let possessive = kind == RepKind::Possessive && self.variant != Variant::Relaxed;
        let (open, iter_open, iter_close) = if possessive {
            ("(?>", "(?>", ")")
        } else {
            ("", "(?:", ")")
        };
        if zero_width {
            let parts = if min >= 1 {
                vec![
                    Work::Text(iter_open),
                    Work::Seq(body, fl),
                    Work::Text(iter_close),
                ]
            } else if kind == RepKind::Lazy {
                vec![Work::Text("(?:|"), Work::Seq(body, fl), Work::Text(")")]
            } else if possessive {
                vec![Work::Text("(?>(?>"), Work::Seq(body, fl), Work::Text(")|)")]
            } else {
                vec![Work::Text("(?:"), Work::Seq(body, fl), Work::Text("|)")]
            };
            Self::push_seq_of(stack, parts);
            return;
        }
        let mut quant = match (min, max) {
            (0, MAXREPEAT) => String::from("*"),
            (1, MAXREPEAT) => String::from("+"),
            (0, 1) => String::from("?"),
            (m, MAXREPEAT) => format!("{{{m},}}"),
            (m, n) if m == n => format!("{{{m}}}"),
            (m, n) => format!("{{{m},{n}}}"),
        };
        if kind == RepKind::Lazy {
            quant.push('?');
        }
        let facts = self.ast.seq_facts(body);
        let exact = self.variant == Variant::Exact;
        let nullable_loop = facts.lo == 0 && max == MAXREPEAT;
        if nullable_loop && kind == RepKind::Greedy && self.variant == Variant::Fast {
            self.cond.span = true;
        }
        let unobservable = !facts.feat.has(Features::CAPTURE)
            || !self
                .ast
                .root_info()
                .feat
                .has(Features::BACKREF.or(Features::COND));
        if exact && nullable_loop && min >= 2 && !possessive && unobservable {
            // fancy-regex compares a nullable loop's epsilon slot at every arrival after the first, also below
            // the minimum, so a slot left over from an earlier entry of the same loop (inside another nullable
            // loop) can leave `X{2,}` after one iteration. CPython runs the first `min` iterations
            // unconditionally and checks emptiness only after that, i.e. `X{m}` followed by a fresh `X*`. The
            // first copy is emitted without capture groups (group numbers are unchanged, as each group still
            // opens once), which is exact whenever no back-reference or conditional can observe a capture.
            let parts = vec![
                Work::Plain(true),
                Work::Text("(?:"),
                Work::Seq(body, fl),
                Work::Plain(false),
                Work::Owned(format!("){{{min}}}(?:")),
                Work::Text(if kind == RepKind::Greedy { "(?=)" } else { "" }),
                Work::Seq(body, fl),
                Work::Text(if kind == RepKind::Lazy { ")*?" } else { ")*" }),
            ];
            Self::push_seq_of(stack, parts);
            return;
        }
        // `min` ≤ 1 only (above): a `{2,}` loop whose captures are observable keeps fancy-regex's own handling.
        let nullable_greedy = nullable_loop && kind == RepKind::Greedy && min <= 1;
        let mark = if exact && nullable_greedy { "(?=)" } else { "" };
        let never_iter = exact
            && min == 0
            && max == MAXREPEAT
            && !nullable_greedy
            && facts.feat.has(Features::CAPTURE);
        let never_rep = exact && min == 0 && kind == RepKind::Lazy;
        let alt_iter = if never_iter {
            format!("|{}", self.never())
        } else {
            String::new()
        };
        let tail = format!(
            "{alt_iter}{iter_close}{quant}{}{}",
            if possessive { ")" } else { "" },
            if never_rep {
                format!("|{})", self.never())
            } else {
                String::new()
            }
        );
        let parts = vec![
            Work::Text(if never_rep { "(?:" } else { "" }),
            Work::Text(open),
            Work::Text(iter_open),
            Work::Text(mark),
            Work::Seq(body, fl),
            Work::Owned(tail),
        ];
        Self::push_seq_of(stack, parts);
    }

    /// §A.6.5.
    fn anchor(&mut self, at: At, fl: Flags) -> &'static str {
        let bytes = self.bytes();
        match at {
            At::BeginningString => {
                self.anchored = true;
                "\\A"
            }
            At::Beginning => {
                self.anchored = true;
                if fl.has(Flags::M) { "(?m:^)" } else { "^" }
            }
            At::EndString => {
                self.anchored = true;
                "\\z"
            }
            At::End => {
                self.anchored = true;
                if fl.has(Flags::M) || self.haystack == Haystack::Line {
                    return "(?m:$)";
                }
                match self.variant {
                    Variant::Fast => {
                        self.cond.nl = true;
                        "\\z"
                    }
                    Variant::Exact => {
                        if bytes {
                            "(?=\\x0A?\\z)"
                        } else {
                            "(?=\\x{A}?\\z)"
                        }
                    }
                    Variant::Relaxed => "(?m:$)",
                }
            }
            At::Boundary | At::NonBoundary => {
                let boundary = at == At::Boundary;
                match self.variant {
                    Variant::Fast => {
                        self.anchored = true;
                        if !bytes {
                            self.cond.ascii = true;
                        }
                        if boundary {
                            "\\b"
                        } else {
                            self.cond.nonempty = true;
                            "\\B"
                        }
                    }
                    Variant::Relaxed => {
                        // Rust's ASCII `\b` on bytes is Python's; its `\B` only adds the empty haystack.
                        if bytes {
                            self.anchored = true;
                            if boundary { "\\b" } else { "\\B" }
                        } else {
                            ""
                        }
                    }
                    Variant::Exact => {
                        self.anchored = true;
                        word_assertion(boundary, self.ascii(fl))
                    }
                }
            }
        }
    }

    /// A literal (§A.6.4) with IGNORECASE (§A.6.6).
    fn literal(&self, c: u32, fl: Flags) -> String {
        if fl.has(Flags::I) {
            let set = self.folded(RangeSet::new(vec![(c, c)]), fl);
            if set.single().is_none() {
                return self.render_class(false, &set, &[], fl);
            }
        }
        let mut out = String::new();
        self.push_unit(&mut out, c);
        out
    }

    /// Clips and folds a literal set per the effective flags.
    fn folded(&self, mut set: RangeSet, fl: Flags) -> RangeSet {
        if !self.bytes() {
            set.clip_surrogates();
        }
        if fl.has(Flags::I) {
            if self.ascii(fl) {
                set.fold_ascii();
            } else {
                set.fold_unicode();
            }
        }
        set
    }

    /// An `IN` set or `NOT_LITERAL` (§A.6.7).
    fn class(&self, negate: bool, items: &[SetItem], fl: Flags) -> String {
        let mut ranges = Vec::new();
        let mut cats: Vec<Cat> = Vec::new();
        for item in items {
            match *item {
                SetItem::Lit(c) => ranges.push((c, c)),
                SetItem::Range(lo, hi) => ranges.push((lo, hi)),
                SetItem::Cat(cat) => {
                    if !cats.contains(&cat) {
                        cats.push(cat);
                    }
                }
            }
        }
        if negate && self.bytes() && fl.has(Flags::L) && fl.has(Flags::I) {
            // `IN_LOC_IGNORE` (`_sre` `charset_loc_ignore`) tests `set(lower(c)) or set(upper(c))` against the
            // *unfolded* negated set: a letter is excluded only when both of its cases are members.
            let raw = RangeSet::new(ranges);
            let member = |b: u32| raw.contains(b) || cats.iter().any(|&cat| ascii_category(cat, b));
            let kept: Vec<(u32, u32)> = (0..=0xFF_u32)
                .filter(|&b| member(b) && member(swapcase_ascii(b)))
                .map(|b| (b, b))
                .collect();
            return self.render_class(true, &RangeSet::new(kept), &[], fl);
        }
        let set = self.folded(RangeSet::new(ranges), fl);
        self.render_class(negate, &set, &cats, fl)
    }

    fn render_class(&self, negate: bool, set: &RangeSet, cats: &[Cat], fl: Flags) -> String {
        if set.is_empty() && cats.is_empty() {
            return String::from(if negate { "(?s:.)" } else { self.never() });
        }
        let mut out = String::from(if negate { "[^" } else { "[" });
        for &(lo, hi) in set.ranges() {
            self.push_unit(&mut out, lo);
            if hi != lo {
                out.push('-');
                self.push_unit(&mut out, hi);
            }
        }
        let ascii = self.ascii(fl);
        for &cat in cats {
            out.push_str(category_text(cat, ascii, self.bytes()));
        }
        out.push(']');
        out
    }

    /// One code unit: raw `[0-9A-Za-z]`, otherwise `\x{H}` (str) or `\xHH` (bytes); a surrogate never matches.
    fn push_unit(&self, out: &mut String, c: u32) {
        if char::from_u32(c).is_some_and(|ch| ch.is_ascii_alphanumeric()) {
            out.push(char::from_u32(c).unwrap_or('?'));
        } else if self.bytes() {
            write!(out, "\\x{c:02X}").unwrap_or_default();
        } else if (0xD800..=0xDFFF).contains(&c) {
            out.push_str(NEVER_STR);
        } else {
            write!(out, "\\x{{{c:X}}}").unwrap_or_default();
        }
    }
}

/// The class contents of a category (§A.6.4 table): Python's `\w` = `[\p{L}\p{N}_]`, `\s` adds U+001C–U+001F,
/// ASCII forms under `(?a)` and for bytes; negated categories are nested negated classes.
pub(crate) fn category_text(cat: Cat, ascii: bool, bytes: bool) -> &'static str {
    match (cat, ascii, bytes) {
        (Cat::Digit, false, _) => "\\d",
        (Cat::NotDigit, false, _) => "\\D",
        (Cat::Space, false, _) => "\\s\\x{1C}-\\x{1F}",
        (Cat::NotSpace, false, _) => "[^\\s\\x{1C}-\\x{1F}]",
        (Cat::Word, false, _) => "\\pL\\pN_",
        (Cat::NotWord, false, _) => "[^\\pL\\pN_]",
        (Cat::Digit, true, _) => "0-9",
        (Cat::NotDigit, true, _) => "[^0-9]",
        (Cat::Space, true, false) => "\\x{9}-\\x{D}\\x{20}",
        (Cat::NotSpace, true, false) => "[^\\x{9}-\\x{D}\\x{20}]",
        (Cat::Space, true, true) => "\\x09-\\x0D\\x20",
        (Cat::NotSpace, true, true) => "[^\\x09-\\x0D\\x20]",
        (Cat::Word, true, _) => "0-9A-Za-z_",
        (Cat::NotWord, true, _) => "[^0-9A-Za-z_]",
    }
}

/// Membership of byte `b` in an ASCII category (bytes patterns; `(?L)` in the C and UTF-8 locales, D4).
fn ascii_category(cat: Cat, b: u32) -> bool {
    let digit = (0x30..=0x39).contains(&b);
    let space = (0x09..=0x0D).contains(&b) || b == 0x20;
    let word = digit || (0x41..=0x5A).contains(&b) || (0x61..=0x7A).contains(&b) || b == 0x5F;
    match cat {
        Cat::Digit => digit,
        Cat::NotDigit => !digit,
        Cat::Space => space,
        Cat::NotSpace => !space,
        Cat::Word => word,
        Cat::NotWord => !word,
    }
}

/// The other case of an ASCII letter; any other byte is itself.
fn swapcase_ascii(b: u32) -> u32 {
    match b {
        0x41..=0x5A => b.saturating_add(0x20),
        0x61..=0x7A => b.saturating_sub(0x20),
        _ => b,
    }
}

/// The exact `\b` / `\B` of §A.6.5 as look-around over Python's word class.
fn word_assertion(boundary: bool, ascii: bool) -> &'static str {
    match (boundary, ascii) {
        (true, false) => "(?:(?<=[\\pL\\pN_])(?![\\pL\\pN_])|(?<![\\pL\\pN_])(?=[\\pL\\pN_]))",
        (true, true) => "(?:(?<=[0-9A-Za-z_])(?![0-9A-Za-z_])|(?<![0-9A-Za-z_])(?=[0-9A-Za-z_]))",
        (false, false) => {
            "(?:(?!\\A)|(?!\\z))(?:(?<=[\\pL\\pN_])(?=[\\pL\\pN_])|(?<![\\pL\\pN_])(?![\\pL\\pN_]))"
        }
        (false, true) => {
            "(?:(?!\\A)|(?!\\z))(?:(?<=[0-9A-Za-z_])(?=[0-9A-Za-z_])|(?<![0-9A-Za-z_])(?![0-9A-Za-z_]))"
        }
    }
}
