//! The CPython 3.12 `re` grammar (01 Addendum §A.6.2): a port of `re/_parser.py` `_parse_sub`, `_parse`,
//! `_escape`, `_class_escape`, `_parse_flags`, `fix_flags` and `parse`, followed by the compile-phase checks
//! of `re/_compiler.py` (look-behind widths, the `(?t)` template rule) computed bottom-up.
//!
//! The recursion of `_parse_sub`/`_parse` is replaced by an explicit frame stack. Python's `nested` argument
//! is tracked exactly and bounded by [`PARSE_NEST_LIMIT`], the depth at which pre-commit 4.6.2 raises
//! `RecursionError` (01 Addendum §A.4: 484 nested groups accepted, 485 rejected).
//!
//! Owner: WP-13.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::ast::{
    Ast, At, Cat, CompileErr, Features, Flags, Info, MAXCODE, MAXGROUPS, MAXREPEAT, MAXWIDTH, Node,
    NodeId, RepKind, Seq, SeqId, SetItem, id_of, ix,
};
use crate::error::{PyReError, PyWarning};
use crate::messages;
use crate::options::Mode;
use crate::tok::{Tok, Tokenizer, unit_char};
use crate::unicode;

/// The largest `nested` argument of a `_parse` frame that pre-commit 4.6.2 survives (01 Addendum §A.4, §A.6.2
/// P6): each group costs two frames (`_parse_sub` + `_parse`), each conditional branch one, so 484 nested groups
/// (innermost `_parse` at `nested = 969`) and 969 nested conditionals (`nested = 970`) compile, while 485 groups
/// (whose `_parse_sub` sits at `nested = 970`) and 970 conditionals raise `RecursionError` (calibrated against
/// CPython 3.12.3 at pre-commit's stack depth, `tests/vectors/nesting.json`).
pub(crate) const PARSE_NEST_LIMIT: u32 = 970;

/// The parsed pattern plus its warnings.
#[derive(Debug)]
pub(crate) struct Parsed {
    /// The AST with bottom-up facts.
    pub(crate) ast: Ast,
    /// `FutureWarning` texts in emission order.
    pub(crate) warnings: Vec<PyWarning>,
}

/// `re.compile(src)` up to (and including) the compile-phase checks; `init` are the flags passed to
/// `re.compile` (pygrep's `re.IGNORECASE`, `re.MULTILINE | re.DOTALL`).
pub(crate) fn parse(src: &str, mode: Mode, init: Flags) -> Result<Parsed, PyReError> {
    let istext = matches!(mode, Mode::Str);
    let units: Vec<u32> = if istext {
        src.chars().map(u32::from).collect()
    } else {
        src.bytes().map(u32::from).collect()
    };
    let t = Tokenizer::new(&units, istext)?;
    let mut p = Parser {
        t,
        nodes: Vec::new(),
        seqs: Vec::new(),
        flags: init,
        groupdict: HashMap::new(),
        closed: vec![false],
        lookbehindgroups: None,
        grouprefpos: IndexMap::new(),
        warnings: Vec::new(),
    };
    let root = p.run()?;
    // `parse` (`re/_parser.py:979-990`): fix_flags, then a stray `)`, then forward conditional references.
    let flags = fix_flags(p.flags, istext)?;
    if p.t.peek().is_some() {
        return Err(p.t.error(messages::UNBALANCED_PARENTHESIS.into(), 0));
    }
    let groups = p.groups();
    for (&g, &pos) in &p.grouprefpos {
        if g >= u64::from(groups) {
            return Err(PyReError::at(
                messages::invalid_group_reference(&g.to_string()),
                pos,
                &units,
            ));
        }
    }
    let mut ast = Ast {
        nodes: p.nodes,
        seqs: p.seqs,
        node_info: Vec::new(),
        seq_info: Vec::new(),
        root,
        flags,
        bytes: !istext,
    };
    compute_info(&mut ast);
    if let Some(err) = ast.root_info().err {
        return Err(match err {
            CompileErr::Template(kind) => PyReError::Re {
                msg: messages::template_operator(kind.opcode()).into(),
                pos: None,
            },
            CompileErr::LooksTooMuchBehind => PyReError::bare(messages::LOOKS_TOO_MUCH_BEHIND),
            CompileErr::LookBehindWidth => PyReError::bare(messages::LOOKBEHIND_FIXED_WIDTH),
        });
    }
    Ok(Parsed {
        ast,
        warnings: p.warnings,
    })
}

/// `fix_flags` (`re/_parser.py:953-967`).
fn fix_flags(flags: Flags, istext: bool) -> Result<Flags, PyReError> {
    if istext {
        if flags.has(Flags::L) {
            return Err(PyReError::Value(messages::LOCALE_WITH_STR.into()));
        }
        if !flags.has(Flags::A) {
            return Ok(flags.or(Flags::U));
        }
        if flags.has(Flags::U) {
            return Err(PyReError::Value(messages::ASCII_AND_UNICODE.into()));
        }
    } else {
        if flags.has(Flags::U) {
            return Err(PyReError::Value(messages::UNICODE_WITH_BYTES.into()));
        }
        if flags.has(Flags::L) && flags.has(Flags::A) {
            return Err(PyReError::Value(messages::ASCII_AND_LOCALE.into()));
        }
    }
    Ok(flags)
}

/// What to do with the sequence a `_parse_sub` frame returns.
#[derive(Debug, Clone, Copy)]
enum SubRet {
    /// The whole pattern.
    Root,
    /// A look-around (`re/_parser.py:761-786`).
    Assert {
        /// `dir == -1`.
        behind: bool,
        /// `ASSERT_NOT`.
        negate: bool,
        /// Position of `(`.
        start: usize,
        /// `lookbehindgroups` was `None` before this look-behind (restore it afterwards).
        reset_lookbehind: bool,
    },
    /// A group (`re/_parser.py:852-872`).
    Group {
        /// Capturing group number.
        gid: Option<u32>,
        /// `(?>…)`.
        atomic: bool,
        /// Scoped flags turned on.
        add: Flags,
        /// Scoped flags turned off.
        del: Flags,
        /// Position of `(`.
        start: usize,
    },
}

/// What to do with the sequence a `_parse` frame returns.
#[derive(Debug, Clone, Copy)]
enum ParseRet {
    /// An alternative of the enclosing `_parse_sub`.
    SubItem,
    /// The yes branch of a conditional.
    CondYes {
        /// The group number.
        gid: u32,
        /// Position of `(`.
        start: usize,
    },
    /// The no branch of a conditional.
    CondNo {
        /// The group number.
        gid: u32,
        /// Position of `(`.
        start: usize,
        /// The yes branch.
        yes: SeqId,
    },
}

/// A `_parse_sub` activation.
#[derive(Debug)]
struct SubFrame {
    items: Vec<SeqId>,
    verbose: bool,
    nested: u32,
    ret: SubRet,
}

/// A `_parse` activation.
#[derive(Debug)]
struct ParseFrame {
    seq: SeqId,
    verbose: bool,
    nested: u32,
    first: bool,
    ret: ParseRet,
}

#[derive(Debug)]
enum Frame {
    Sub(SubFrame),
    Parse(ParseFrame),
}

/// A finished frame's result, delivered to the frame below it.
#[derive(Debug, Clone, Copy)]
enum Done {
    Sub(SeqId, SubRet),
    Parse(SeqId, ParseRet),
}

/// What a `_parse` step asks the driver to do.
#[derive(Debug)]
enum Step {
    /// Push the current frame back, then this child.
    Descend(Frame),
    /// The frame is complete.
    Finish,
}

/// The parser state (`re._parser.State` plus the tokenizer and the arena).
#[derive(Debug)]
struct Parser<'a> {
    t: Tokenizer<'a>,
    nodes: Vec<Node>,
    seqs: Vec<Seq>,
    /// `state.flags`.
    flags: Flags,
    /// `state.groupdict`.
    groupdict: HashMap<String, u32>,
    /// `state.groupwidths[gid] is not None`, i.e. the group is closed (index 0 = group 0).
    closed: Vec<bool>,
    /// `state.lookbehindgroups`.
    lookbehindgroups: Option<u32>,
    /// `state.grouprefpos` (insertion order).
    grouprefpos: IndexMap<u64, usize>,
    warnings: Vec<PyWarning>,
}

impl Parser<'_> {
    /// `state.groups`.
    fn groups(&self) -> u32 {
        id_of(self.closed.len()).unwrap_or(u32::MAX)
    }

    /// `state.checkgroup(gid)`.
    fn checkgroup(&self, gid: u32) -> bool {
        gid < self.groups() && self.closed.get(ix(gid)).copied().unwrap_or(false)
    }

    /// `state.checklookbehindgroup(gid, source)` (`re/_parser.py:103-109`).
    fn checklookbehindgroup(&self, gid: u32) -> Result<(), PyReError> {
        if let Some(lb) = self.lookbehindgroups {
            if !self.checkgroup(gid) {
                return Err(self.t.error(messages::OPEN_GROUP.into(), 0));
            }
            if gid >= lb {
                return Err(self.t.error(messages::SAME_LOOKBEHIND.into(), 0));
            }
        }
        Ok(())
    }

    fn new_seq(&mut self) -> Result<SeqId, PyReError> {
        let id = id_of(self.seqs.len()).ok_or_else(too_large)?;
        self.seqs.push(Seq::default());
        Ok(id)
    }

    fn new_node(&mut self, node: Node) -> Result<NodeId, PyReError> {
        let id = id_of(self.nodes.len()).ok_or_else(too_large)?;
        self.nodes.push(node);
        Ok(id)
    }

    fn seq_mut(&mut self, id: SeqId) -> Option<&mut Seq> {
        self.seqs.get_mut(ix(id))
    }

    fn push_item(&mut self, seq: SeqId, node: Node) -> Result<(), PyReError> {
        let id = self.new_node(node)?;
        if let Some(s) = self.seq_mut(seq) {
            s.items.push(id);
        }
        Ok(())
    }

    fn warn(&mut self, msg: String) {
        self.warnings.push(PyWarning {
            msg: msg.into_boxed_str(),
        });
    }

    /// A child frame, unless it would exceed pre-commit's recursion budget.
    fn sub_frame(verbose: bool, nested: u32, ret: SubRet) -> Result<Frame, PyReError> {
        // `_parse_sub` immediately calls `_parse(nested + 1)`.
        if nested >= PARSE_NEST_LIMIT {
            return Err(PyReError::Recursion);
        }
        Ok(Frame::Sub(SubFrame {
            items: Vec::new(),
            verbose,
            nested,
            ret,
        }))
    }

    fn parse_frame(
        &mut self,
        verbose: bool,
        nested: u32,
        first: bool,
        ret: ParseRet,
    ) -> Result<Frame, PyReError> {
        if nested > PARSE_NEST_LIMIT {
            return Err(PyReError::Recursion);
        }
        let seq = self.new_seq()?;
        Ok(Frame::Parse(ParseFrame {
            seq,
            verbose,
            nested,
            first,
            ret,
        }))
    }

    /// Drives the frame stack; returns the top-level sequence.
    fn run(&mut self) -> Result<SeqId, PyReError> {
        let verbose = self.flags.has(Flags::X);
        let mut stack = vec![Self::sub_frame(verbose, 0, SubRet::Root)?];
        let mut done: Option<Done> = None;
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Sub(mut f) => {
                    if let Some(Done::Parse(seq, ParseRet::SubItem)) = done.take() {
                        f.items.push(seq);
                        if !self.t.eat('|')? {
                            let seq = self.finish_sub(&f)?;
                            if matches!(f.ret, SubRet::Root) {
                                return Ok(seq);
                            }
                            done = Some(Done::Sub(seq, f.ret));
                            continue;
                        }
                        if f.nested == 0 {
                            f.verbose = self.flags.has(Flags::X);
                        }
                    }
                    // items.append(_parse(source, state, verbose, nested + 1, not nested and not items))
                    let first = f.nested == 0 && f.items.is_empty();
                    let child = self.parse_frame(
                        f.verbose,
                        f.nested.saturating_add(1),
                        first,
                        ParseRet::SubItem,
                    )?;
                    stack.push(Frame::Sub(f));
                    stack.push(child);
                }
                Frame::Parse(mut f) => {
                    if let Some(d) = done.take()
                        && let Some(child) = self.resume(&f, d)?
                    {
                        stack.push(Frame::Parse(f));
                        stack.push(child);
                        continue;
                    }
                    match self.step(&mut f)? {
                        Step::Descend(child) => {
                            stack.push(Frame::Parse(f));
                            stack.push(child);
                        }
                        Step::Finish => {
                            self.unpack_groups(f.seq);
                            done = Some(Done::Parse(f.seq, f.ret));
                        }
                    }
                }
            }
        }
        Err(too_large())
    }

    /// The end of `_parse_sub`: one alternative is returned as is, several become a `BRANCH`.
    fn finish_sub(&mut self, f: &SubFrame) -> Result<SeqId, PyReError> {
        if let [only] = f.items.as_slice() {
            return Ok(*only);
        }
        let seq = self.new_seq()?;
        self.push_item(seq, Node::Branch(f.items.clone().into_boxed_slice()))?;
        Ok(seq)
    }

    /// "unpack non-capturing groups" (`re/_parser.py:883-889`).
    fn unpack_groups(&mut self, seq: SeqId) {
        let items = self
            .seqs
            .get(ix(seq))
            .map(|s| s.items.clone())
            .unwrap_or_default();
        let plain = |n: &Node| matches!(n, Node::Group { gid: None, add, del, .. } if add.0 == 0 && del.0 == 0);
        if !items
            .iter()
            .any(|&id| self.nodes.get(ix(id)).is_some_and(plain))
        {
            return;
        }
        let mut out = Vec::new();
        for id in items {
            match self.nodes.get(ix(id)) {
                Some(Node::Group {
                    gid: None,
                    add,
                    del,
                    body,
                }) if add.0 == 0 && del.0 == 0 => {
                    out.extend_from_slice(
                        self.seqs
                            .get(ix(*body))
                            .map_or(&[][..], |s| s.items.as_slice()),
                    );
                }
                _ => out.push(id),
            }
        }
        if let Some(s) = self.seq_mut(seq) {
            s.items = out;
        }
    }

    /// Completes the construct that descended into a child frame, then lets `_parse` continue.
    fn resume(&mut self, f: &ParseFrame, done: Done) -> Result<Option<Frame>, PyReError> {
        match done {
            Done::Sub(
                body,
                SubRet::Assert {
                    behind,
                    negate,
                    start,
                    reset_lookbehind,
                },
            ) => {
                if behind && reset_lookbehind {
                    self.lookbehindgroups = None;
                }
                self.expect_close(start)?;
                self.push_item(
                    f.seq,
                    Node::Assert {
                        behind,
                        negate,
                        body,
                    },
                )?;
            }
            Done::Sub(
                body,
                SubRet::Group {
                    gid,
                    atomic,
                    add,
                    del,
                    start,
                },
            ) => {
                self.expect_close(start)?;
                if let Some(g) = gid
                    && let Some(c) = self.closed.get_mut(ix(g))
                {
                    *c = true;
                }
                let node = if atomic {
                    Node::Atomic(body)
                } else {
                    Node::Group {
                        gid,
                        add,
                        del,
                        body,
                    }
                };
                self.push_item(f.seq, node)?;
            }
            Done::Parse(yes, ParseRet::CondYes { gid, start }) => {
                if self.t.eat('|')? {
                    let ret = ParseRet::CondNo { gid, start, yes };
                    return self
                        .parse_frame(f.verbose, f.nested.saturating_add(1), false, ret)
                        .map(Some);
                }
                self.finish_cond(f, gid, start, yes, None)?;
            }
            Done::Parse(no, ParseRet::CondNo { gid, start, yes }) => {
                if self.t.next_is('|') {
                    return Err(self.t.error(messages::COND_TOO_MANY_BRANCHES.into(), 0));
                }
                self.finish_cond(f, gid, start, yes, Some(no))?;
            }
            Done::Sub(_, SubRet::Root) | Done::Parse(_, ParseRet::SubItem) => {
                return Err(too_large());
            }
        }
        Ok(None)
    }

    fn finish_cond(
        &mut self,
        f: &ParseFrame,
        gid: u32,
        start: usize,
        yes: SeqId,
        no: Option<SeqId>,
    ) -> Result<(), PyReError> {
        self.expect_close(start)?;
        self.push_item(f.seq, Node::GroupRefExists { gid, yes, no })
    }

    /// `if not source.match(")"): raise source.error("missing ), unterminated subpattern", …)`.
    fn expect_close(&mut self, start: usize) -> Result<(), PyReError> {
        if !self.t.eat(')')? {
            let off = self.t.tell().saturating_sub(start);
            return Err(self.t.error(messages::UNTERMINATED_SUBPATTERN.into(), off));
        }
        Ok(())
    }

    /// The `_parse` loop body until the frame ends or must descend (`re/_parser.py:523-881`).
    fn step(&mut self, f: &mut ParseFrame) -> Result<Step, PyReError> {
        loop {
            let Some(this) = self.t.peek() else {
                return Ok(Step::Finish);
            };
            if this.one_of("|)") {
                return Ok(Step::Finish);
            }
            self.t.get()?;
            if f.verbose {
                if this.one_of(" \t\n\r\u{b}\u{c}") {
                    continue;
                }
                if this.is('#') {
                    loop {
                        match self.t.get()? {
                            None => break,
                            Some(t) if t.is('\n') => break,
                            Some(_) => {}
                        }
                    }
                    continue;
                }
            }
            if this.esc {
                let node = self.escape(this)?;
                self.push_item(f.seq, node)?;
            } else if !this.one_of(".\\[{()*+?^$|") {
                self.push_item(f.seq, Node::Lit(this.c))?;
            } else if this.is('[') {
                let node = self.class()?;
                self.push_item(f.seq, node)?;
            } else if this.one_of("*+?{") {
                self.repeat(f, this)?;
            } else if this.is('.') {
                self.push_item(f.seq, Node::Any)?;
            } else if this.is('(') {
                if let Some(child) = self.group(f)? {
                    return Ok(Step::Descend(child));
                }
            } else if this.is('^') {
                self.push_item(f.seq, Node::At(At::Beginning))?;
            } else if this.is('$') {
                self.push_item(f.seq, Node::At(At::End))?;
            }
        }
    }

    /// A repeat (`re/_parser.py:639-704`).
    fn repeat(&mut self, f: &ParseFrame, this: Tok) -> Result<(), PyReError> {
        let here = self.t.tell();
        let (min, max) = if this.is('?') {
            (0, 1)
        } else if this.is('*') {
            (0, MAXREPEAT)
        } else if this.is('+') {
            (1, MAXREPEAT)
        } else {
            // `{`
            if self.t.next_is('}') {
                self.push_item(f.seq, Node::Lit(this.c))?;
                return Ok(());
            }
            let mut lo = String::new();
            while self.t.peek().is_some_and(Tok::is_digit) {
                if let Some(d) = self.t.get()? {
                    d.push_to(&mut lo);
                }
            }
            let hi = if self.t.eat(',')? {
                let mut hi = String::new();
                while self.t.peek().is_some_and(Tok::is_digit) {
                    if let Some(d) = self.t.get()? {
                        d.push_to(&mut hi);
                    }
                }
                hi
            } else {
                lo.clone()
            };
            if !self.t.eat('}')? {
                self.push_item(f.seq, Node::Lit(this.c))?;
                self.t.seek(here)?;
                return Ok(());
            }
            let mut min = 0_u32;
            let mut max = MAXREPEAT;
            if !lo.is_empty() {
                min = repeat_count(&lo)?;
            }
            if !hi.is_empty() {
                max = repeat_count(&hi)?;
                if max < min {
                    let off = self.t.tell().saturating_sub(here);
                    return Err(self.t.error(messages::MIN_GT_MAX.into(), off));
                }
            }
            (min, max)
        };
        let off = self
            .t
            .tell()
            .saturating_sub(here)
            .saturating_add(this.len());
        let Some(last) = self
            .seqs
            .get(ix(f.seq))
            .and_then(|s| s.items.last().copied())
        else {
            return Err(self.t.error(messages::NOTHING_TO_REPEAT.into(), off));
        };
        match self.nodes.get(ix(last)) {
            Some(Node::At(_)) | None => {
                return Err(self.t.error(messages::NOTHING_TO_REPEAT.into(), off));
            }
            Some(Node::Repeat { .. }) => {
                return Err(self.t.error(messages::MULTIPLE_REPEAT.into(), off));
            }
            _ => {}
        }
        let body = match self.nodes.get(ix(last)) {
            Some(Node::Group {
                gid: None,
                add,
                del,
                body,
            }) if add.0 == 0 && del.0 == 0 => *body,
            _ => {
                let s = self.new_seq()?;
                if let Some(seq) = self.seq_mut(s) {
                    seq.items.push(last);
                }
                s
            }
        };
        let kind = if self.t.eat('?')? {
            RepKind::Lazy
        } else if self.t.eat('+')? {
            RepKind::Possessive
        } else {
            RepKind::Greedy
        };
        let id = self.new_node(Node::Repeat {
            kind,
            min,
            max,
            body,
        })?;
        if let Some(s) = self.seq_mut(f.seq)
            && let Some(slot) = s.items.last_mut()
        {
            *slot = id;
        }
        Ok(())
    }

    /// `_escape` (`re/_parser.py:372-447`).
    fn escape(&mut self, this: Tok) -> Result<Node, PyReError> {
        let c = unit_char(this.c);
        match c {
            'A' => return Ok(Node::At(At::BeginningString)),
            'b' => return Ok(Node::At(At::Boundary)),
            'B' => return Ok(Node::At(At::NonBoundary)),
            'Z' => return Ok(Node::At(At::EndString)),
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                let cat = category(c).unwrap_or(Cat::Digit);
                return Ok(Node::In {
                    negate: false,
                    items: Box::new([SetItem::Cat(cat)]),
                });
            }
            _ => {}
        }
        if let Some(v) = simple_escape(c) {
            return Ok(Node::Lit(v));
        }
        if let Some(v) = self.code_escape(this)? {
            return Ok(Node::Lit(v));
        }
        if c == '0' {
            let mut esc = this.text();
            self.t.get_while(2, Tok::is_octal, &mut esc)?;
            return Ok(Node::Lit(octal_value(&esc)));
        }
        if c.is_ascii_digit() {
            let mut esc = this.text();
            if self.t.peek().is_some_and(Tok::is_digit) {
                if let Some(d) = self.t.get()? {
                    d.push_to(&mut esc);
                }
                let both_octal = esc
                    .chars()
                    .skip(1)
                    .take(2)
                    .all(|d| ('0'..='7').contains(&d));
                if both_octal && self.t.peek().is_some_and(Tok::is_octal) {
                    if let Some(d) = self.t.get()? {
                        d.push_to(&mut esc);
                    }
                    let v = octal_value(&esc);
                    if v > 0o377 {
                        return Err(self
                            .t
                            .error(messages::octal_out_of_range(&esc), esc.chars().count()));
                    }
                    return Ok(Node::Lit(v));
                }
            }
            let digits = esc.get(1..).unwrap_or_default();
            let group: u32 = digits.parse().unwrap_or(u32::MAX);
            if group < self.groups() {
                if !self.checkgroup(group) {
                    return Err(self
                        .t
                        .error(messages::OPEN_GROUP.into(), esc.chars().count()));
                }
                self.checklookbehindgroup(group)?;
                return Ok(Node::GroupRef(group));
            }
            let off = esc.chars().count().saturating_sub(1);
            return Err(self.t.error(messages::invalid_group_reference(digits), off));
        }
        if c.is_ascii_alphabetic() {
            return Err(self.t.error(messages::bad_escape(&this.text()), 2));
        }
        Ok(Node::Lit(this.c))
    }

    /// The escapes shared by `_escape` and `_class_escape`: `\x`, `\u`, `\U`, `\N` (Ok(None) = not one).
    fn code_escape(&mut self, this: Tok) -> Result<Option<u32>, PyReError> {
        let c = unit_char(this.c);
        let digits = match c {
            'x' => 2,
            'u' if self.t.istext => 4,
            'U' if self.t.istext => 8,
            'N' if self.t.istext => return self.named_escape().map(Some),
            _ => return Ok(None),
        };
        let mut esc = this.text();
        let got = self.t.get_while(digits, Tok::is_hex, &mut esc)?;
        if got != digits {
            return Err(self
                .t
                .error(messages::incomplete_escape(&esc), esc.chars().count()));
        }
        let v = esc
            .get(2..)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .unwrap_or(u32::MAX);
        if c == 'U' && v > 0x10_FFFF {
            return Err(self
                .t
                .error(messages::bad_escape(&esc), esc.chars().count()));
        }
        Ok(Some(v))
    }

    /// `\N{name}` (`re/_parser.py:402-413`; 01 Addendum §A.5.4).
    fn named_escape(&mut self) -> Result<u32, PyReError> {
        if !self.t.eat('{')? {
            return Err(self.t.error(messages::MISSING_LBRACE.into(), 0));
        }
        let (name, len) = self.t.get_until('}', "character name")?;
        match crate::names::lookup(&name) {
            Ok(Some(v)) => Ok(v),
            Ok(None) => {
                let quoted = snp_pycompat::repr::repr_str(&name);
                Err(self
                    .t
                    .error(messages::undefined_name(&quoted), len.saturating_add(4)))
            }
            Err(e) => Err(e),
        }
    }

    /// `_class_escape` (`re/_parser.py:312-370`): a literal or a category.
    fn class_escape(&mut self, this: Tok) -> Result<SetItem, PyReError> {
        let c = unit_char(this.c);
        if let Some(v) = simple_escape(c) {
            return Ok(SetItem::Lit(v));
        }
        if c == 'b' {
            return Ok(SetItem::Lit(0x08));
        }
        if let Some(cat) = category(c) {
            return Ok(SetItem::Cat(cat));
        }
        if let Some(v) = self.code_escape(this)? {
            return Ok(SetItem::Lit(v));
        }
        if ('0'..='7').contains(&c) {
            let mut esc = this.text();
            self.t.get_while(2, Tok::is_octal, &mut esc)?;
            let v = octal_value(&esc);
            if v > 0o377 {
                return Err(self
                    .t
                    .error(messages::octal_out_of_range(&esc), esc.chars().count()));
            }
            return Ok(SetItem::Lit(v));
        }
        if c.is_ascii_digit() || c.is_ascii_alphabetic() {
            return Err(self.t.error(messages::bad_escape(&this.text()), 2));
        }
        Ok(SetItem::Lit(this.c))
    }

    /// A character set (`re/_parser.py:550-637`).
    fn class(&mut self) -> Result<Node, PyReError> {
        let here = self.t.tell().saturating_sub(1);
        if self.t.next_is('[') {
            let pos = self.t.tell();
            self.warn(messages::nested_set(pos));
        }
        let negate = self.t.eat('^')?;
        let mut set: Vec<SetItem> = Vec::new();
        loop {
            let Some(this) = self.t.get()? else {
                let off = self.t.tell().saturating_sub(here);
                return Err(self.t.error(messages::UNTERMINATED_SET.into(), off));
            };
            if this.is(']') && !set.is_empty() {
                break;
            }
            let code1 = if this.esc {
                self.class_escape(this)?
            } else {
                if !set.is_empty()
                    && this.one_of("-&~|")
                    && self.t.peek().is_some_and(|n| !n.esc && n.c == this.c)
                {
                    let kind = match unit_char(this.c) {
                        '-' => "difference",
                        '&' => "intersection",
                        '~' => "symmetric difference",
                        _ => "union",
                    };
                    let pos = self.t.tell().saturating_sub(1);
                    self.warn(messages::set_operation(kind, pos));
                }
                SetItem::Lit(this.c)
            };
            if self.t.eat('-')? {
                let Some(that) = self.t.get()? else {
                    let off = self.t.tell().saturating_sub(here);
                    return Err(self.t.error(messages::UNTERMINATED_SET.into(), off));
                };
                if that.is(']') {
                    set.push(code1);
                    set.push(SetItem::Lit(u32::from('-')));
                    break;
                }
                let code2 = if that.esc {
                    self.class_escape(that)?
                } else {
                    if that.is('-') {
                        let pos = self.t.tell().saturating_sub(2);
                        self.warn(messages::set_operation("difference", pos));
                    }
                    SetItem::Lit(that.c)
                };
                let off = this.len().saturating_add(1).saturating_add(that.len());
                let (SetItem::Lit(lo), SetItem::Lit(hi)) = (code1, code2) else {
                    return Err(self
                        .t
                        .error(messages::bad_range(&this.text(), &that.text()), off));
                };
                if hi < lo {
                    return Err(self
                        .t
                        .error(messages::bad_range(&this.text(), &that.text()), off));
                }
                set.push(SetItem::Range(lo, hi));
            } else {
                set.push(code1);
            }
        }
        // `_uniq`, then the single-literal optimisation.
        let mut uniq: Vec<SetItem> = Vec::new();
        for item in set {
            if !uniq.contains(&item) {
                uniq.push(item);
            }
        }
        if let [SetItem::Lit(c)] = uniq.as_slice() {
            return Ok(if negate {
                Node::NotLit(*c)
            } else {
                Node::Lit(*c)
            });
        }
        Ok(Node::In {
            negate,
            items: uniq.into_boxed_slice(),
        })
    }

    /// `(`: groups and extensions (`re/_parser.py:709-872`). Returns a child frame when the group has a body.
    fn group(&mut self, f: &mut ParseFrame) -> Result<Option<Frame>, PyReError> {
        let start = self.t.tell().saturating_sub(1);
        let mut capture = true;
        let mut atomic = false;
        let mut name: Option<(String, usize)> = None;
        let mut add = Flags::empty();
        let mut del = Flags::empty();
        let child_nested = f.nested.saturating_add(1);
        if self.t.eat('?')? {
            let Some(char) = self.t.get()? else {
                return Err(self.t.error(messages::UNEXPECTED_END.into(), 0));
            };
            if char.is('P') {
                if self.t.eat('<')? {
                    let n = self.t.get_until('>', "group name")?;
                    self.checkgroupname(&n.0, n.1, 1)?;
                    name = Some(n);
                } else if self.t.eat('=')? {
                    let (n, len) = self.t.get_until(')', "group name")?;
                    self.checkgroupname(&n, len, 1)?;
                    let off = len.saturating_add(1);
                    let Some(&gid) = self.groupdict.get(&n) else {
                        return Err(self
                            .t
                            .error(messages::unknown_group_name(&Self::quote(&n)), off));
                    };
                    if !self.checkgroup(gid) {
                        return Err(self.t.error(messages::OPEN_GROUP.into(), off));
                    }
                    self.checklookbehindgroup(gid)?;
                    self.push_item(f.seq, Node::GroupRef(gid))?;
                    return Ok(None);
                } else {
                    let Some(char) = self.t.get()? else {
                        return Err(self.t.error(messages::UNEXPECTED_END.into(), 0));
                    };
                    let rest = format!("P{}", char.text());
                    return Err(self.t.error(
                        messages::unknown_extension(&rest),
                        char.len().saturating_add(2),
                    ));
                }
            } else if char.is(':') {
                capture = false;
            } else if char.is('#') {
                loop {
                    if self.t.peek().is_none() {
                        let off = self.t.tell().saturating_sub(start);
                        return Err(self.t.error(messages::UNTERMINATED_COMMENT.into(), off));
                    }
                    if self.t.get()?.is_some_and(|t| t.is(')')) {
                        break;
                    }
                }
                return Ok(None);
            } else if char.one_of("=!<") {
                let mut behind = false;
                let mut negate = char.is('!');
                let mut reset_lookbehind = false;
                if char.is('<') {
                    let Some(c2) = self.t.get()? else {
                        return Err(self.t.error(messages::UNEXPECTED_END.into(), 0));
                    };
                    if !c2.one_of("=!") {
                        let rest = format!("<{}", c2.text());
                        return Err(self.t.error(
                            messages::unknown_extension(&rest),
                            c2.len().saturating_add(2),
                        ));
                    }
                    behind = true;
                    negate = c2.is('!');
                    if self.lookbehindgroups.is_none() {
                        self.lookbehindgroups = Some(self.groups());
                        reset_lookbehind = true;
                    }
                }
                let ret = SubRet::Assert {
                    behind,
                    negate,
                    start,
                    reset_lookbehind,
                };
                return Self::sub_frame(f.verbose, child_nested, ret).map(Some);
            } else if char.is('(') {
                return self.conditional(f, start).map(Some);
            } else if char.is('>') {
                capture = false;
                atomic = true;
            } else if (!char.esc && Flags::from_letter(char.c).is_some()) || char.is('-') {
                match self.parse_flags(char)? {
                    None => {
                        if !f.first || !self.seq_is_empty(f.seq) {
                            let off = self.t.tell().saturating_sub(start);
                            return Err(self
                                .t
                                .error(messages::GLOBAL_FLAGS_NOT_AT_START.into(), off));
                        }
                        f.verbose = self.flags.has(Flags::X);
                        return Ok(None);
                    }
                    Some((a, d)) => {
                        add = a;
                        del = d;
                        capture = false;
                    }
                }
            } else {
                return Err(self.t.error(
                    messages::unknown_extension(&char.text()),
                    char.len().saturating_add(1),
                ));
            }
        }
        let gid = if capture {
            Some(self.opengroup(name.as_ref())?)
        } else {
            None
        };
        let sub_verbose = (f.verbose || add.has(Flags::X)) && !del.has(Flags::X);
        let ret = SubRet::Group {
            gid,
            atomic,
            add,
            del,
            start,
        };
        Self::sub_frame(sub_verbose, child_nested, ret).map(Some)
    }

    fn seq_is_empty(&self, seq: SeqId) -> bool {
        self.seqs.get(ix(seq)).is_none_or(|s| s.items.is_empty())
    }

    /// `(?(cond)yes|no)` up to the yes branch (`re/_parser.py:788-817`).
    fn conditional(&mut self, f: &ParseFrame, start: usize) -> Result<Frame, PyReError> {
        let (condname, len) = self.t.get_until(')', "group name")?;
        let off = len.saturating_add(1);
        let gid = if !condname.is_empty() && condname.bytes().all(|b| b.is_ascii_digit()) {
            let trimmed = condname.trim_start_matches('0');
            let value: u64 = if trimmed.len() > 18 {
                u64::MAX
            } else {
                trimmed.parse().unwrap_or(0)
            };
            if value == 0 {
                return Err(self.t.error(messages::BAD_GROUP_NUMBER.into(), off));
            }
            if value >= MAXGROUPS {
                let shown = if trimmed.is_empty() { "0" } else { trimmed };
                return Err(self.t.error(messages::invalid_group_reference(shown), off));
            }
            let pos = self.t.tell().saturating_sub(off);
            self.grouprefpos.entry(value).or_insert(pos);
            u32::try_from(value).unwrap_or(u32::MAX)
        } else {
            self.checkgroupname(&condname, len, 1)?;
            let Some(&gid) = self.groupdict.get(&condname) else {
                return Err(self
                    .t
                    .error(messages::unknown_group_name(&Self::quote(&condname)), off));
            };
            gid
        };
        self.checklookbehindgroup(gid)?;
        self.parse_frame(
            f.verbose,
            f.nested.saturating_add(1),
            false,
            ParseRet::CondYes { gid, start },
        )
    }

    /// `state.opengroup(name)` (`re/_parser.py:86-97`).
    fn opengroup(&mut self, name: Option<&(String, usize)>) -> Result<u32, PyReError> {
        let gid = self.groups();
        if u64::from(gid) >= MAXGROUPS {
            return Err(too_large());
        }
        self.closed.push(false);
        if let Some((n, len)) = name {
            if let Some(&ogid) = self.groupdict.get(n) {
                let msg = messages::redefinition(&Self::quote(n), gid, ogid);
                return Err(self.t.error(msg, len.saturating_add(1)));
            }
            self.groupdict.insert(n.clone(), gid);
        }
        Ok(gid)
    }

    /// Python `%r` of a name (str patterns) — bytes messages are backslash-replaced by `error`.
    fn quote(s: &str) -> String {
        snp_pycompat::repr::repr_str(s)
    }

    /// `Tokenizer.checkgroupname` (`re/_parser.py:304-310`).
    fn checkgroupname(&self, name: &str, len: usize, offset: usize) -> Result<(), PyReError> {
        let off = len.saturating_add(offset);
        if !self.t.istext && !name.is_ascii() {
            return Err(self
                .t
                .error(messages::bad_group_name(&ascii_repr(name)), off));
        }
        if !unicode::is_identifier(name) {
            return Err(self
                .t
                .error(messages::bad_group_name(&Self::quote(name)), off));
        }
        Ok(())
    }

    /// `_parse_flags` (`re/_parser.py:893-951`): `None` for global flags (applied to the state).
    fn parse_flags(&mut self, first: Tok) -> Result<Option<(Flags, Flags)>, PyReError> {
        let istext = self.t.istext;
        let mut add = Flags::empty();
        let mut del = Flags::empty();
        let mut char = Some(first);
        if !first.is('-') {
            while let Some(c) = char {
                let flag = Flags::from_letter(c.c).unwrap_or_default();
                if istext && c.is('L') {
                    return Err(self.t.error(messages::FLAG_L_WITH_STR.into(), 0));
                }
                if !istext && c.is('u') {
                    return Err(self.t.error(messages::FLAG_U_WITH_BYTES.into(), 0));
                }
                add = add.or(flag);
                if flag.has(Flags::TYPE) && add.and(Flags::TYPE) != flag {
                    return Err(self.t.error(messages::FLAGS_INCOMPATIBLE.into(), 0));
                }
                char = self.t.get()?;
                let Some(next) = char else {
                    return Err(self.t.error(messages::MISSING_DASH_COLON_PAREN.into(), 0));
                };
                if next.one_of(")-:") {
                    break;
                }
                if next.esc || Flags::from_letter(next.c).is_none() {
                    let msg = if is_alpha_token(next) {
                        messages::UNKNOWN_FLAG
                    } else {
                        messages::MISSING_DASH_COLON_PAREN
                    };
                    return Err(self.t.error(msg.into(), next.len()));
                }
            }
        }
        let Some(mut c) = char else {
            return Err(too_large());
        };
        if c.is(')') {
            self.flags = self.flags.or(add);
            return Ok(None);
        }
        if add.has(Flags::GLOBAL) {
            return Err(self.t.error(messages::TURN_ON_GLOBAL.into(), 1));
        }
        if c.is('-') {
            let Some(next) = self.t.get()? else {
                return Err(self.t.error(messages::MISSING_FLAG.into(), 0));
            };
            if next.esc || Flags::from_letter(next.c).is_none() {
                let msg = if is_alpha_token(next) {
                    messages::UNKNOWN_FLAG
                } else {
                    messages::MISSING_FLAG
                };
                return Err(self.t.error(msg.into(), next.len()));
            }
            c = next;
            loop {
                let flag = Flags::from_letter(c.c).unwrap_or_default();
                if flag.has(Flags::TYPE) {
                    return Err(self.t.error(messages::TURN_OFF_TYPE.into(), 0));
                }
                del = del.or(flag);
                let Some(next) = self.t.get()? else {
                    return Err(self.t.error(messages::MISSING_COLON.into(), 0));
                };
                if next.is(':') {
                    break;
                }
                if next.esc || Flags::from_letter(next.c).is_none() {
                    let msg = if is_alpha_token(next) {
                        messages::UNKNOWN_FLAG
                    } else {
                        messages::MISSING_COLON
                    };
                    return Err(self.t.error(msg.into(), next.len()));
                }
                c = next;
            }
        }
        if del.has(Flags::GLOBAL) {
            return Err(self.t.error(messages::TURN_OFF_GLOBAL.into(), 1));
        }
        if add.and(del).0 != 0 {
            return Err(self.t.error(messages::FLAG_ON_AND_OFF.into(), 1));
        }
        Ok(Some((add, del)))
    }
}

/// `char.isalpha()` for a token (an escape token contains `\`, which is not alphabetic).
fn is_alpha_token(t: Tok) -> bool {
    !t.esc && unicode::is_alpha(unit_char(t.c))
}

/// `ascii(name)` (`%a`): only reached for bytes patterns, whose names are Latin-1.
fn ascii_repr(name: &str) -> String {
    messages::backslashreplace(&snp_pycompat::repr::repr_str(name))
}

/// `int(digits)` for a repeat bound, or `OverflowError` at `MAXREPEAT` (`re/_parser.py:668-675`).
fn repeat_count(digits: &str) -> Result<u32, PyReError> {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.len() > 10 {
        return Err(PyReError::Overflow);
    }
    let v: u64 = if trimmed.is_empty() {
        0
    } else {
        trimmed.parse().map_err(|_| PyReError::Overflow)?
    };
    if v >= u64::from(MAXREPEAT) {
        return Err(PyReError::Overflow);
    }
    u32::try_from(v).map_err(|_| PyReError::Overflow)
}

/// `int(escape[1:], 8)` for `\0`, `\NNN` and class octal escapes (at most three digits).
fn octal_value(esc: &str) -> u32 {
    esc.chars().skip(1).fold(0_u32, |acc, d| {
        acc.saturating_mul(8)
            .saturating_add(d.to_digit(8).unwrap_or(0))
    })
}

/// `ESCAPES` without `\b` (`re/_parser.py:31-40`), whose meaning depends on the context.
fn simple_escape(c: char) -> Option<u32> {
    Some(match c {
        'a' => 0x07,
        'f' => 0x0C,
        'n' => 0x0A,
        'r' => 0x0D,
        't' => 0x09,
        'v' => 0x0B,
        '\\' => 0x5C,
        _ => return None,
    })
}

/// The `IN` categories of `CATEGORIES` (`re/_parser.py:42-53`).
fn category(c: char) -> Option<Cat> {
    Some(match c {
        'd' => Cat::Digit,
        'D' => Cat::NotDigit,
        's' => Cat::Space,
        'S' => Cat::NotSpace,
        'w' => Cat::Word,
        'W' => Cat::NotWord,
        _ => return None,
    })
}

/// An internal invariant failed or an arena outgrew `u32`: reported as an engine limit, never a panic.
fn too_large() -> PyReError {
    PyReError::Unsupported(messages::engine_limit("pattern too large").into_boxed_str())
}

/// Cost estimate of one atom for [`Info::est`].
///
/// The unit approximates a microsecond of `regex` compile time for one expanded copy of the atom (measured with
/// regex 1.13: an ASCII literal costs ~2.5 µs, a Unicode IGNORECASE literal ~12 µs, `.` or a negated class over
/// UTF-8 ~15 µs, a Unicode `\d` ~60–200 µs, a Unicode `\w`/`\W` ~0.7–1 ms); repeats multiply it (see
/// [`crate::compile`]'s budget).
fn atom_cost(node: &Node, unicode: bool, ignorecase: bool) -> u64 {
    let lit: u64 = if unicode && ignorecase { 12 } else { 2 };
    let class_cost = |items: &[SetItem], negate: bool| -> u64 {
        let mut cost: u64 = if negate && unicode { 15 } else { 2 };
        for item in items {
            cost = cost.saturating_add(match item {
                SetItem::Lit(_) => lit,
                SetItem::Range(..) => {
                    if unicode {
                        10
                    } else {
                        2
                    }
                }
                SetItem::Cat(_) if !unicode => 3,
                SetItem::Cat(Cat::Digit | Cat::NotDigit) => 150,
                SetItem::Cat(Cat::Space | Cat::NotSpace) => 15,
                SetItem::Cat(Cat::Word | Cat::NotWord) => 1000,
            });
        }
        cost
    };
    match node {
        Node::In { items, negate } => class_cost(items, *negate),
        Node::NotLit(_) | Node::Any if unicode => 15,
        _ => lit,
    }
}

/// Fills `node_info`/`seq_info` bottom-up (`SubPattern.getwidth`, `_compile`'s errors in compile order).
fn compute_info(ast: &mut Ast) {
    let template = ast.flags.has(Flags::T);
    let unicode = !ast.bytes && !ast.flags.has(Flags::A);
    let ignorecase = ast.flags.has(Flags::I);
    let n = ast.nodes.len();
    let mut node_info: Vec<Info> = Vec::new();
    let mut seq_info: Vec<Option<Info>> = vec![None; ast.seqs.len()];
    let mut group_width: HashMap<u32, (u128, u128)> = HashMap::new();
    for i in 0..n {
        let Some(node) = ast.nodes.get(i) else { break };
        let mut seq = |sid: SeqId, node_info: &[Info]| -> Info {
            if let Some(Some(info)) = seq_info.get(ix(sid)) {
                return *info;
            }
            let info = ast
                .seqs
                .get(ix(sid))
                .map(|s| {
                    s.items.iter().fold(Info::default(), |acc, &it| {
                        acc.then(node_info.get(ix(it)).copied().unwrap_or_default())
                    })
                })
                .unwrap_or_default()
                .clamped();
            if let Some(slot) = seq_info.get_mut(ix(sid)) {
                *slot = Some(info);
            }
            info
        };
        let info = match node {
            Node::Lit(_) | Node::NotLit(_) | Node::Any | Node::In { .. } => Info {
                lo: 1,
                hi: 1,
                err: None,
                feat: Features::default(),
                est: atom_cost(node, unicode, ignorecase),
            },
            Node::At(_) => Info {
                est: 1,
                ..Info::default()
            },
            Node::Branch(alts) => {
                let mut lo = MAXWIDTH;
                let mut hi = 0;
                let mut acc = Info::default();
                for &a in alts {
                    let s = seq(a, &node_info);
                    lo = lo.min(s.lo);
                    hi = hi.max(s.hi);
                    acc = acc.then(s);
                }
                Info { lo, hi, ..acc }
            }
            Node::Group { gid, body, .. } => {
                let s = seq(*body, &node_info);
                if let Some(g) = gid {
                    group_width.insert(*g, (s.lo, s.hi));
                    Info {
                        feat: s.feat.or(Features::CAPTURE),
                        ..s
                    }
                } else {
                    s
                }
            }
            Node::Repeat {
                kind,
                min,
                max,
                body,
            } => {
                let s = seq(*body, &node_info);
                let own = template.then_some(CompileErr::Template(*kind));
                let lo = s.lo.saturating_mul(u128::from(*min));
                let hi = if *max == MAXREPEAT && s.hi > 0 {
                    MAXWIDTH
                } else {
                    s.hi.saturating_mul(u128::from(*max))
                };
                let copies = if *max == MAXREPEAT {
                    u64::from(*min).saturating_add(1)
                } else {
                    u64::from(*max).max(1)
                };
                let feat = if matches!(kind, RepKind::Possessive) {
                    s.feat.or(Features::POSSESSIVE)
                } else {
                    s.feat
                };
                Info {
                    lo,
                    hi,
                    err: own.or(s.err),
                    feat,
                    est: s.est.saturating_mul(copies).saturating_add(1),
                }
            }
            Node::GroupRef(g) => {
                let (lo, hi) = group_width.get(g).copied().unwrap_or((0, 0));
                Info {
                    lo,
                    hi,
                    err: None,
                    feat: Features::BACKREF,
                    est: 1,
                }
            }
            Node::GroupRefExists { yes, no, .. } => {
                let y = seq(*yes, &node_info);
                let (lo, hi, acc) = match no {
                    Some(no) => {
                        let z = seq(*no, &node_info);
                        (y.lo.min(z.lo), y.hi.max(z.hi), y.then(z))
                    }
                    None => (0, y.hi, y),
                };
                Info {
                    lo,
                    hi,
                    feat: acc.feat.or(Features::COND),
                    ..acc
                }
            }
            Node::Assert { behind, body, .. } => {
                let s = seq(*body, &node_info);
                let own = if !*behind {
                    None
                } else if s.lo > MAXCODE {
                    Some(CompileErr::LooksTooMuchBehind)
                } else if s.lo != s.hi {
                    Some(CompileErr::LookBehindWidth)
                } else {
                    None
                };
                Info {
                    lo: 0,
                    hi: 0,
                    err: own.or(s.err),
                    feat: s.feat.or(Features::LOOK),
                    est: s.est.saturating_add(1),
                }
            }
            Node::Atomic(body) => {
                let s = seq(*body, &node_info);
                Info {
                    feat: s.feat.or(Features::ATOMIC),
                    ..s
                }
            }
        };
        node_info.push(info);
    }
    let root = ast.root;
    let root_info = ast
        .seqs
        .get(ix(root))
        .map(|s| {
            s.items.iter().fold(Info::default(), |acc, &it| {
                acc.then(node_info.get(ix(it)).copied().unwrap_or_default())
            })
        })
        .unwrap_or_default()
        .clamped();
    if let Some(slot) = seq_info.get_mut(ix(root)) {
        *slot = Some(root_info);
    }
    // Sequences never reached from a node (orphans of the unpacking) still get facts, for completeness.
    let mut filled: Vec<Info> = Vec::new();
    for (sid, slot) in seq_info.iter().enumerate() {
        let info = slot.unwrap_or_else(|| {
            ast.seqs
                .get(sid)
                .map(|s| {
                    s.items.iter().fold(Info::default(), |acc, &it| {
                        acc.then(node_info.get(ix(it)).copied().unwrap_or_default())
                    })
                })
                .unwrap_or_default()
                .clamped()
        });
        filled.push(info);
    }
    ast.node_info = node_info;
    ast.seq_info = filled;
}
