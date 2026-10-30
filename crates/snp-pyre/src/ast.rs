//! The parsed pattern: an arena of nodes and sequences mirroring CPython 3.12's `re._parser.SubPattern`
//! opcodes (`re/_parser.py`, `re/_constants.py`), plus the per-node facts computed bottom-up after the parse
//! (`SubPattern.getwidth`, the compile-phase checks of `re/_compiler.py:_compile`, the tier-2 features).
//!
//! Every walk over the arena is iterative: nodes are created after their children, so index order is a valid
//! bottom-up order, and top-down walks use explicit stacks. No input can exhaust the native stack.
//!
//! Owner: WP-13.

/// Python's `MAXREPEAT` (`_sre.MAXREPEAT` on 64-bit builds): a repeat bound of this value means "unbounded".
pub(crate) const MAXREPEAT: u32 = u32::MAX;
/// Python's `MAXWIDTH` (`re/_parser.py:73`): `1 << 64`, the clamp of `SubPattern.getwidth`.
pub(crate) const MAXWIDTH: u128 = 1 << 64;
/// Python's `MAXCODE` for `CODESIZE == 4` (`re/_compiler.py`): the largest look-behind width.
pub(crate) const MAXCODE: u128 = 0xFFFF_FFFF;
/// Python's `MAXGROUPS` (`_sre.MAXGROUPS` on 64-bit builds).
pub(crate) const MAXGROUPS: u64 = 1_073_741_823;

/// The `re` flag bits (`re/_constants.py` `SRE_FLAG_*`), as far as the parser and the lowering need them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct Flags(pub(crate) u16);

impl Flags {
    /// `re.IGNORECASE` (`i`).
    pub(crate) const I: Self = Self(1);
    /// `re.LOCALE` (`L`).
    pub(crate) const L: Self = Self(1 << 1);
    /// `re.MULTILINE` (`m`).
    pub(crate) const M: Self = Self(1 << 2);
    /// `re.DOTALL` (`s`).
    pub(crate) const S: Self = Self(1 << 3);
    /// `re.VERBOSE` (`x`).
    pub(crate) const X: Self = Self(1 << 4);
    /// `re.ASCII` (`a`).
    pub(crate) const A: Self = Self(1 << 5);
    /// `re.TEMPLATE` (`t`, 3.12 only).
    pub(crate) const T: Self = Self(1 << 6);
    /// `re.UNICODE` (`u`).
    pub(crate) const U: Self = Self(1 << 7);
    /// `TYPE_FLAGS` (`re/_parser.py:68`).
    pub(crate) const TYPE: Self = Self(Self::A.0 | Self::L.0 | Self::U.0);
    /// `GLOBAL_FLAGS` (`re/_parser.py:69`; `DEBUG` cannot be written inline).
    pub(crate) const GLOBAL: Self = Self::T;

    /// No bits.
    pub(crate) const fn empty() -> Self {
        Self(0)
    }

    /// Any bit of `other` set in `self`.
    pub(crate) const fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// `self | other`.
    pub(crate) const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// `self & !other`.
    pub(crate) const fn minus(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// `self & other`.
    pub(crate) const fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The flag of an inline flag letter (`re/_parser.py:55-66`).
    pub(crate) const fn from_letter(c: u32) -> Option<Self> {
        match c {
            0x69 => Some(Self::I), // i
            0x4C => Some(Self::L), // L
            0x6D => Some(Self::M), // m
            0x73 => Some(Self::S), // s
            0x78 => Some(Self::X), // x
            0x61 => Some(Self::A), // a
            0x74 => Some(Self::T), // t
            0x75 => Some(Self::U), // u
            _ => None,
        }
    }

    /// `_combine_flags` (`re/_compiler.py:30-34`): the flags inside `(?add-del:…)`.
    pub(crate) const fn combine(self, add: Self, del: Self) -> Self {
        let base = if add.has(Self::TYPE) {
            self.minus(Self::TYPE)
        } else {
            self
        };
        base.or(add).minus(del)
    }
}

/// `AT` codes (`re/_constants.py`): anchors and word boundaries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum At {
    /// `^` (`AT_BEGINNING`).
    Beginning,
    /// `\A` (`AT_BEGINNING_STRING`).
    BeginningString,
    /// `$` (`AT_END`).
    End,
    /// `\Z` (`AT_END_STRING`).
    EndString,
    /// `\b` (`AT_BOUNDARY`).
    Boundary,
    /// `\B` (`AT_NON_BOUNDARY`).
    NonBoundary,
}

/// `CATEGORY` codes (`re/_constants.py`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cat {
    /// `\d`.
    Digit,
    /// `\D`.
    NotDigit,
    /// `\s`.
    Space,
    /// `\S`.
    NotSpace,
    /// `\w`.
    Word,
    /// `\W`.
    NotWord,
}

/// One member of an `IN` set (`re/_parser.py:552-631`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SetItem {
    /// `(LITERAL, c)`.
    Lit(u32),
    /// `(RANGE, (lo, hi))`.
    Range(u32, u32),
    /// `(CATEGORY, cat)`.
    Cat(Cat),
}

/// The three repeat opcodes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum RepKind {
    /// `MAX_REPEAT` (greedy).
    Greedy,
    /// `MIN_REPEAT` (lazy, `*?`).
    Lazy,
    /// `POSSESSIVE_REPEAT` (`*+`, 3.11+).
    Possessive,
}

impl RepKind {
    /// The opcode name Python prints in `internal: unsupported template operator …`.
    pub(crate) const fn opcode(self) -> &'static str {
        match self {
            Self::Greedy => "MAX_REPEAT",
            Self::Lazy => "MIN_REPEAT",
            Self::Possessive => "POSSESSIVE_REPEAT",
        }
    }
}

/// An index into [`Ast::nodes`].
pub(crate) type NodeId = u32;
/// An index into [`Ast::seqs`].
pub(crate) type SeqId = u32;

/// One opcode of a `SubPattern`.
#[derive(Clone, Debug)]
pub(crate) enum Node {
    /// `LITERAL c` (a code point for str patterns, a byte for bytes patterns).
    Lit(u32),
    /// `NOT_LITERAL c` (`[^c]`).
    NotLit(u32),
    /// `ANY` (`.`).
    Any,
    /// `IN` (a class; categories outside classes are one-member classes, as in Python).
    In {
        /// `NEGATE` was the first member.
        negate: bool,
        /// The other members, deduplicated in order.
        items: Box<[SetItem]>,
    },
    /// `AT`.
    At(At),
    /// `BRANCH` (`a|b`): two or more alternatives.
    Branch(Box<[SeqId]>),
    /// `SUBPATTERN`: a capturing group, or a non-capturing group that changes flags.
    Group {
        /// The group number of a capturing group.
        gid: Option<u32>,
        /// Flags turned on.
        add: Flags,
        /// Flags turned off.
        del: Flags,
        /// The contents.
        body: SeqId,
    },
    /// `MAX_REPEAT` / `MIN_REPEAT` / `POSSESSIVE_REPEAT`.
    Repeat {
        /// The opcode.
        kind: RepKind,
        /// Lower bound.
        min: u32,
        /// Upper bound ([`MAXREPEAT`] = unbounded).
        max: u32,
        /// The repeated item.
        body: SeqId,
    },
    /// `GROUPREF` (`\1`, `(?P=name)`).
    GroupRef(u32),
    /// `GROUPREF_EXISTS` (`(?(1)yes|no)`).
    GroupRefExists {
        /// The group number.
        gid: u32,
        /// The yes branch.
        yes: SeqId,
        /// The no branch, if written.
        no: Option<SeqId>,
    },
    /// `ASSERT` / `ASSERT_NOT`.
    Assert {
        /// Look-behind (`dir == -1`).
        behind: bool,
        /// `ASSERT_NOT`.
        negate: bool,
        /// The asserted pattern.
        body: SeqId,
    },
    /// `ATOMIC_GROUP` (`(?>…)`).
    Atomic(SeqId),
}

/// The first error `re/_compiler.py:_compile` would raise, in compile (pre-)order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CompileErr {
    /// `internal: unsupported template operator MAX_REPEAT|MIN_REPEAT|POSSESSIVE_REPEAT`.
    Template(RepKind),
    /// `looks too much behind`.
    LooksTooMuchBehind,
    /// `look-behind requires fixed-width pattern`.
    LookBehindWidth,
}

/// Tier-2 features and other facts collected bottom-up.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct Features(pub(crate) u8);

impl Features {
    /// A look-ahead or look-behind.
    pub(crate) const LOOK: Self = Self(1);
    /// A back-reference.
    pub(crate) const BACKREF: Self = Self(1 << 1);
    /// A conditional (`GROUPREF_EXISTS`).
    pub(crate) const COND: Self = Self(1 << 2);
    /// An atomic group.
    pub(crate) const ATOMIC: Self = Self(1 << 3);
    /// A possessive repeat.
    pub(crate) const POSSESSIVE: Self = Self(1 << 4);
    /// A capturing group.
    pub(crate) const CAPTURE: Self = Self(1 << 5);
    /// Everything that needs the backtracking engine (01 Addendum §A.6.9).
    pub(crate) const TIER2: Self =
        Self(Self::LOOK.0 | Self::BACKREF.0 | Self::COND.0 | Self::ATOMIC.0 | Self::POSSESSIVE.0);

    /// Any bit of `other` set.
    pub(crate) const fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// `self | other`.
    pub(crate) const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Bottom-up facts of a node or a sequence.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct Info {
    /// `getwidth()[0]` (clamped to [`MAXWIDTH`] at sequence boundaries).
    pub(crate) lo: u128,
    /// `getwidth()[1]`.
    pub(crate) hi: u128,
    /// The first compile-phase error in compile order.
    pub(crate) err: Option<CompileErr>,
    /// Features.
    pub(crate) feat: Features,
    /// A saturating estimate of the expanded size (atoms after unrolling counted repeats).
    pub(crate) est: u64,
}

impl Info {
    /// Sequential composition (`SubPattern.getwidth` sums; first error wins).
    pub(crate) fn then(self, next: Self) -> Self {
        Self {
            lo: self.lo.saturating_add(next.lo),
            hi: self.hi.saturating_add(next.hi),
            err: self.err.or(next.err),
            feat: self.feat.or(next.feat),
            est: self.est.saturating_add(next.est),
        }
    }

    /// The clamp applied when a `SubPattern` caches its width.
    pub(crate) fn clamped(self) -> Self {
        Self {
            lo: self.lo.min(MAXWIDTH),
            hi: self.hi.min(MAXWIDTH),
            ..self
        }
    }
}

/// A `SubPattern`: a sequence of nodes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Seq {
    /// The items, in order.
    pub(crate) items: Vec<NodeId>,
}

/// A parsed pattern.
#[derive(Clone, Debug)]
pub(crate) struct Ast {
    /// The node arena (children before parents).
    pub(crate) nodes: Vec<Node>,
    /// The sequence arena.
    pub(crate) seqs: Vec<Seq>,
    /// Per-node facts (same indices as `nodes`).
    pub(crate) node_info: Vec<Info>,
    /// Per-sequence facts (same indices as `seqs`).
    pub(crate) seq_info: Vec<Info>,
    /// The top-level sequence.
    pub(crate) root: SeqId,
    /// The global flags after `fix_flags`, including the compile options.
    pub(crate) flags: Flags,
    /// True for bytes patterns.
    pub(crate) bytes: bool,
}

impl Ast {
    /// The node `id`.
    pub(crate) fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(ix(id))
    }

    /// The items of sequence `id` (empty for an unknown id).
    pub(crate) fn seq(&self, id: SeqId) -> &[NodeId] {
        self.seqs.get(ix(id)).map_or(&[], |s| s.items.as_slice())
    }

    /// The facts of sequence `id`.
    pub(crate) fn seq_facts(&self, id: SeqId) -> Info {
        self.seq_info.get(ix(id)).copied().unwrap_or_default()
    }

    /// The facts of the whole pattern.
    pub(crate) fn root_info(&self) -> Info {
        self.seq_facts(self.root)
    }
}

/// A `u32` arena index as `usize` (lossless on every supported target).
pub(crate) fn ix(id: u32) -> usize {
    usize::try_from(id).unwrap_or(usize::MAX)
}

/// A `usize` length as an arena index (`None` past `u32::MAX`).
pub(crate) fn id_of(len: usize) -> Option<u32> {
    u32::try_from(len).ok()
}
