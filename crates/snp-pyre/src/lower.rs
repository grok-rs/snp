//! Lowering of the parsed pattern to engine syntax (01 Addendum §A.6.4–§A.6.8, §A.6.11).
//!
//! One emitter, three variants:
//! * [`Variant::Fast`]: tier-1 (`regex`) text with the cheaper anchor forms of §A.6.5 and the condition bits
//!   that tell when a haystack needs the exact variant;
//! * [`Variant::Exact`]: the canonical forms (`$` → `(?=\n?\z)`, look-around `\b`/`\B`) for `fancy-regex`;
//! * [`Variant::Relaxed`]: the tier-1 prefilter of a tier-2 pattern (look-around → nothing, back-reference →
//!   `(?s:.*)`, conditional → alternation, atomic/possessive → plain, `$` → `(?m:$)`), whose language is a
//!   superset of the pattern's under search semantics.
//!
//! Every literal except `[0-9A-Za-z]` is hex-escaped (str `\x{H}`, bytes `\xHH`); no inline flag is emitted
//! except `(?s:.)`, `(?m:^)`, `(?m:$)` and `(?i:\N)`; capturing groups keep Python's numbering. The walk is
//! iterative (explicit work stack).
//!
//! Owner: WP-13.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::ast::{Ast, At, Cat, Flags, MAXREPEAT, Node, NodeId, RepKind, SeqId, SetItem};
use crate::fold::RangeSet;
use crate::options::Haystack;

/// Which text to produce.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Tier 1 with haystack-dependent fast forms.
    Fast,
    /// Canonical forms for the backtracking engine.
    Exact,
    /// The tier-1 prefilter of a tier-2 pattern.
    Relaxed,
}

/// When a haystack must use the exact variant instead of the fast one (§A.6.5).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cond {
    /// A non-MULTILINE `$` became `\z`: haystacks ending with `\n` need the exact form.
    pub(crate) nl: bool,
    /// A str `\b`/`\B` became Rust's: non-ASCII haystacks need the exact form.
    pub(crate) ascii: bool,
    /// A `\B` became Rust's: the empty haystack needs the exact form (Python's `\B` is false there).
    pub(crate) nonempty: bool,
}

impl Cond {
    /// No condition: the fast form is exact for every haystack.
    pub(crate) const fn is_none(self) -> bool {
        !self.nl && !self.ascii && !self.nonempty
    }

    /// `use_exact` of §A.6.5 for haystack `h`.
    pub(crate) fn needs_exact(self, h: &[u8]) -> bool {
        (self.nl && h.last() == Some(&b'\n'))
            || (self.nonempty && h.is_empty())
            || (self.ascii && !h.is_ascii())
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

    /// §A.6.8.
    fn repeat(
        &self,
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
        let tail = format!("{iter_close}{quant}{}", if possessive { ")" } else { "" });
        let parts = vec![
            Work::Text(open),
            Work::Text(iter_open),
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
