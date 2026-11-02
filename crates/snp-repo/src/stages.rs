//! The legacy stage names a manifest uses, for `clientlib.warn_for_stages_on_repo_init` (`clientlib.py:84-109`).
//!
//! pre-commit re-loads the new clone's `.pre-commit-hooks.yaml` with `MINIMAL_MANIFEST_SCHEMA` (a list of maps with a
//! string `id` and an optional `stages` list of strings) through `cfgv.load_from_filename` (`open(…,
//! encoding='utf-8').read()`: strict UTF-8 and universal newlines) and `yaml.load(…, Loader=CSafeLoader)`, collects
//! the names that `transform_stage` would rename (`commit`, `push`, `merge-commit`) in first-seen order, and logs one
//! warning; any load or validation error is swallowed ("they'll get a better error message when it actually
//! loads!").
//!
//! The §1.2 DAG gives snp-repo no YAML parser (snp-config only exposes the *transformed* stages), so this module
//! carries a small composer for the YAML 1.1 subset manifests are written in: an optional `%YAML 1.1` directive,
//! `---` / `...` markers and comments; block sequences (compact, indentless and with the node on the next line) and
//! block mappings; flow sequences and mappings; plain (multi-line), single- and double-quoted scalars; anchors on a
//! node, the `!!str` / `!!seq` / `!!map` tags; block scalars (as opaque strings). Plain scalars are typed with
//! PyYAML's implicit resolvers, duplicate keys keep the last value. Every construct outside the subset (aliases,
//! other tags, complex keys, tabs, multi-line quoted scalars, a second document) and every input libyaml rejects
//! (`mapping values are not allowed here`, a nested block sequence where a scalar was expected, a stray indentation
//! level, an unknown escape, a non-printable character) yields `None`: no warning. The asymmetry is deliberate — an
//! unfollowed construct can only lose a warning, never invent one. `tests/vectors/legacy_stages.tsv` holds
//! pre-commit 4.7.0's own verdict on hand-written edge cases, every upstream pre-commit-hooks manifest and a
//! generated differential corpus.
//!
//! Owner: WP-31.

/// The names `clientlib._STAGES` renames, as `&'static str` for the warning text.
const LEGACY: [&str; 3] = ["commit", "push", "merge-commit"];

/// Nesting bound (the composer recurses per collection level).
const MAX_DEPTH: u32 = 64;

/// The nodes alias copies may add in total (PyYAML shares them; a copy must stay bounded).
const MAX_ALIAS_NODES: usize = 100_000;

/// The anchors a manifest may define (no real manifest has any).
const MAX_ANCHORS: usize = 256;

/// libyaml's limit on a simple key (`could not find expected ':'` beyond it).
const MAX_SIMPLE_KEY: usize = 1024;

/// The legacy stage names of `manifest` in first-seen order, deduplicated; empty when there are none or when the
/// minimal manifest load would fail.
pub(crate) fn legacy_stages(manifest: &[u8]) -> Vec<&'static str> {
    let Ok(text) = std::str::from_utf8(manifest) else {
        return Vec::new();
    };
    if !text.contains("commit") && !text.contains("push") && !text.contains('\\') {
        return Vec::new();
    }
    let Some(text) = universal_newlines(text) else {
        return Vec::new();
    };
    let index = LineIndex::new(text.as_bytes());
    let Some(root) = Parser::new(text.as_bytes(), &index).document() else {
        return Vec::new();
    };
    minimal_schema(&root).unwrap_or_default()
}

/// Python's text-mode read (`\r\n` and a lone `\r` become `\n`) plus libyaml's reader: NEL, LS and PS are line
/// breaks as well, and `None` for a character libyaml rejects.
fn universal_newlines(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            // libyaml's own line breaks (NEL, LS, PS) are breaks too.
            '\n' | '\u{85}' | '\u{2028}' | '\u{2029}' => out.push('\n'),
            '\t' => out.push(c),
            ' '..='~' | '\u{a0}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'.. => {
                if c == '\u{feff}' {
                    // A BOM inside the stream: libyaml's handling differs by position; not followed.
                    return None;
                }
                out.push(c);
            }
            // Control characters (a reader error).
            _ => return None,
        }
    }
    Some(out)
}

/// A composed node.
#[derive(Debug, Clone)]
enum Node {
    /// An empty node (`null`).
    Null,
    /// A scalar: its value and whether it is plain (typed by the implicit resolvers) or a string.
    Scalar(String, bool),
    /// A block scalar (always a string; its value is never a stage name this module looks for).
    Opaque,
    /// A sequence.
    Seq(Vec<Self>),
    /// A mapping in document order (duplicates kept; the last one wins on lookup).
    Map(Vec<(Self, Self)>),
}

impl Node {
    /// The value when the node constructs to `str`.
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar(s, false) => Some(s),
            Self::Scalar(s, true) => is_plain_str(s).then_some(s.as_str()),
            Self::Opaque => Some("\0"),
            Self::Null | Self::Seq(_) | Self::Map(_) => None,
        }
    }

    /// Whether the node is hashable as a mapping key (`found unhashable key` otherwise).
    const fn hashable(&self) -> bool {
        !matches!(self, Self::Seq(_) | Self::Map(_))
    }
}

/// `MINIMAL_MANIFEST_SCHEMA` and the stage collection; `None` when validation fails.
fn minimal_schema(root: &Node) -> Option<Vec<&'static str>> {
    let Node::Seq(hooks) = root else {
        return None;
    };
    let mut found: Vec<&'static str> = Vec::new();
    for hook in hooks {
        let Node::Map(entries) = hook else {
            return None;
        };
        let get = |key: &str| {
            entries
                .iter()
                .rev()
                .find(|(k, _)| k.as_str() == Some(key))
                .map(|(_, v)| v)
        };
        get("id")?.as_str()?;
        let Some(stages) = get("stages") else {
            continue;
        };
        let Node::Seq(items) = stages else {
            return None;
        };
        for item in items {
            let s = item.as_str()?;
            if let Some(legacy) = LEGACY.iter().find(|l| **l == s)
                && !found.contains(legacy)
            {
                found.push(legacy);
            }
        }
    }
    Some(found)
}

/// Whether `b` separates tokens (`None` = end of input).
const fn blank(b: Option<u8>) -> bool {
    matches!(b, None | Some(b' ' | b'\t' | b'\n'))
}

/// Whether `b` is a flow indicator.
const fn flow_indicator(b: Option<u8>) -> bool {
    matches!(b, Some(b',' | b'[' | b']' | b'{' | b'}'))
}

/// libyaml's anchor-name characters (`IS_ALPHA`).
const fn anchor_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// `(height, size)` of a node, for the alias bounds.
fn measure(n: &Node) -> (u32, usize) {
    match n {
        Node::Null | Node::Scalar(..) | Node::Opaque => (0, 1),
        Node::Seq(items) => items.iter().fold((1, 1), |(h, s), i| {
            let (ih, is) = measure(i);
            (h.max(ih.saturating_add(1)), s.saturating_add(is))
        }),
        Node::Map(entries) => entries.iter().fold((1, 1), |(h, s), (k, v)| {
            let (kh, ks) = measure(k);
            let (vh, vs) = measure(v);
            (
                h.max(kh.max(vh).saturating_add(1)),
                s.saturating_add(ks).saturating_add(vs),
            )
        }),
    }
}

/// Line starts and indentation ends, so column and indentation queries stay logarithmic on hostile input.
#[derive(Debug)]
struct LineIndex {
    /// The offset of every line start.
    starts: Vec<usize>,
    /// The offset of the first non-space byte of every line.
    indent_ends: Vec<usize>,
}

impl LineIndex {
    fn new(s: &[u8]) -> Self {
        let mut starts = vec![0];
        for (i, &b) in s.iter().enumerate() {
            if b == b'\n' {
                starts.push(i.saturating_add(1));
            }
        }
        let indent_ends = starts
            .iter()
            .map(|&st| {
                let spaces = s
                    .get(st..)
                    .map_or(0, |rest| rest.iter().take_while(|&&c| c == b' ').count());
                st.saturating_add(spaces)
            })
            .collect();
        Self {
            starts,
            indent_ends,
        }
    }

    /// The index of the line holding `pos`.
    fn line_of(&self, pos: usize) -> usize {
        self.starts
            .partition_point(|&st| st <= pos)
            .saturating_sub(1)
    }
}

/// The node properties read before a node.
#[derive(Debug, Default)]
struct Props<'s> {
    /// `&name`.
    anchor: Option<&'s [u8]>,
    /// `!tag`.
    tag: Option<&'s [u8]>,
}

/// The composer (see the module docs); every method returns `None` for an error or an unfollowed construct.
struct Parser<'s> {
    /// The normalised text.
    s: &'s [u8],
    /// Its lines.
    index: &'s LineIndex,
    /// The cursor.
    pos: usize,
    /// The collection nesting.
    depth: u32,
    /// The anchored nodes (a duplicate anchor is a `ComposerError`).
    anchors: Vec<(&'s [u8], Node, u32)>,
    /// Nodes copied for aliases so far.
    copied: usize,
}

impl<'s> Parser<'s> {
    const fn new(s: &'s [u8], index: &'s LineIndex) -> Self {
        Self {
            s,
            index,
            pos: 0,
            depth: 0,
            anchors: Vec::new(),
            copied: 0,
        }
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.s.get(i).copied()
    }

    fn peek(&self) -> Option<u8> {
        self.at(self.pos)
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.at(self.pos.saturating_add(off))
    }

    fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    /// The start of the line holding `pos`.
    fn line_start(&self, pos: usize) -> usize {
        self.index
            .starts
            .get(self.index.line_of(pos))
            .copied()
            .unwrap_or(0)
    }

    /// The end of the line holding `pos` (the `\n` or the end of input).
    fn line_end(&self, pos: usize) -> usize {
        self.s
            .get(pos..)
            .and_then(|b| b.iter().position(|&c| c == b'\n'))
            .map_or(self.s.len(), |i| pos.saturating_add(i))
    }

    /// The column (in characters, as libyaml counts) of `pos`.
    fn col(&self, pos: usize) -> i64 {
        let start = self.line_start(pos);
        let n = self
            .s
            .get(start..pos)
            .map_or(0, |b| b.iter().filter(|&&c| c & 0xc0 != 0x80).count());
        i64::try_from(n).unwrap_or(i64::MAX)
    }

    /// Whether only spaces precede `pos` on its line (a tab there is indentation libyaml rejects).
    fn in_indentation(&self, pos: usize) -> bool {
        self.index
            .indent_ends
            .get(self.index.line_of(pos))
            .is_some_and(|&end| pos <= end)
    }

    /// A `---` or `...` marker at `pos` (column 0, followed by a blank).
    fn marker_at(&self, pos: usize) -> Option<u8> {
        if pos != 0 && pos.checked_sub(1).and_then(|p| self.at(p)) != Some(b'\n') {
            return None;
        }
        let m = self.s.get(pos..pos.saturating_add(3))?;
        let after = self.at(pos.saturating_add(3));
        (matches!(m, b"---" | b"...") && blank(after)).then(|| m.first().copied().unwrap_or(b'-'))
    }

    /// Skips spaces, tabs between tokens, comments and line breaks. `None` for a tab in the indentation (or on an
    /// otherwise blank line) and for a `#` glued to the previous token.
    fn skip_to_token(&mut self) -> Option<()> {
        loop {
            match self.peek() {
                Some(b' ' | b'\n') => self.bump(),
                Some(b'\t') => {
                    if self.in_indentation(self.pos) {
                        return None;
                    }
                    self.bump();
                }
                Some(b'#') => {
                    let prev = self.pos.checked_sub(1).and_then(|p| self.at(p));
                    if !blank(prev) {
                        return None;
                    }
                    self.pos = self.line_end(self.pos);
                }
                _ => return Some(()),
            }
        }
    }

    /// A copy positioned at the cursor (lookahead).
    fn probe(&self) -> Self {
        Self {
            s: self.s,
            index: self.index,
            pos: self.pos,
            depth: self.depth,
            anchors: Vec::new(),
            copied: 0,
        }
    }

    /// The next token's position, or `None` (an error on the way).
    fn next_token(&self) -> Option<usize> {
        let mut p = self.probe();
        p.skip_to_token()?;
        Some(p.pos)
    }

    /// Whether the next token is on the cursor's line.
    fn token_on_this_line(&self) -> Option<bool> {
        let next = self.next_token()?;
        Some(self.at(next).is_some() && self.line_start(next) == self.line_start(self.pos))
    }

    /// The rest of the line holds only blanks and a comment.
    fn rest_clean(&self) -> Option<()> {
        (!self.token_on_this_line()?).then_some(())
    }

    /// One document: a directive, markers, the root node; a second document is an error.
    fn document(&mut self) -> Option<Node> {
        self.skip_to_token()?;
        let mut explicit = false;
        let mut version = false;
        while self.peek() == Some(b'%') {
            let end = self.line_end(self.pos);
            let line = self.s.get(self.pos..end)?;
            let line = line
                .iter()
                .position(|&c| c == b'#')
                .and_then(|i| line.get(..i))
                .unwrap_or(line)
                .trim_ascii();
            if matches!(line, b"%YAML 1.1" | b"%YAML 1.2") && !version {
                version = true;
            } else if !line.starts_with(b"%TAG ") {
                return None;
            }
            self.pos = end;
            self.skip_to_token()?;
            explicit = true;
        }
        let root = if self.marker_at(self.pos) == Some(b'-') {
            self.pos = self.pos.saturating_add(3);
            if self.token_on_this_line()? {
                // `--- [ … ]`: only a flow collection is followed on the marker line.
                self.pos = self.next_token()?;
                if !matches!(self.peek(), Some(b'[' | b'{')) {
                    return None;
                }
                let n = self.flow()?;
                self.rest_clean()?;
                n
            } else {
                self.node(-1, false, false)?
            }
        } else if explicit {
            return None;
        } else {
            self.node(-1, false, false)?
        };
        self.skip_to_token()?;
        if self.marker_at(self.pos) == Some(b'.') {
            self.pos = self.pos.saturating_add(3);
            self.rest_clean()?;
            self.skip_to_token()?;
        }
        self.peek().is_none().then_some(root)
    }

    /// Enters a nesting level.
    fn enter(&mut self) -> Option<()> {
        self.depth = self.depth.saturating_add(1);
        (self.depth <= MAX_DEPTH).then_some(())
    }

    /// Leaves a nesting level.
    const fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// A block node whose parent collection sits at column `parent`. `indentless`: a block sequence at the parent's
    /// own column is this node (a mapping value); `value_inline`: the node starts on its key's line (no block
    /// collection may start there).
    fn node(&mut self, parent: i64, indentless: bool, value_inline: bool) -> Option<Node> {
        let start_line = self.line_start(self.pos);
        self.skip_to_token()?;
        let crossed = self.line_start(self.pos) != start_line;
        let value_inline = value_inline && !crossed;
        let Some(b) = self.peek() else {
            return Some(Node::Null);
        };
        if self.marker_at(self.pos).is_some() {
            return Some(Node::Null);
        }
        let c = self.col(self.pos);
        let dash = b == b'-' && blank(self.peek_at(1));
        if crossed && c <= parent {
            if indentless && c == parent && dash {
                return self.block_seq(c, true);
            }
            return Some(Node::Null);
        }
        match b {
            b'-' if dash => {
                if value_inline {
                    // `key: - a`: "sequence entries are not allowed here".
                    return None;
                }
                self.block_seq(c, false)
            }
            b'[' | b'{' => {
                let n = self.flow()?;
                self.rest_clean()?;
                Some(n)
            }
            b'&' | b'!' => {
                self.enter()?;
                let n = self.with_properties(c, parent, indentless, value_inline);
                self.leave();
                n
            }
            b'*' => {
                let n = self.alias()?;
                if self.value_indicator() {
                    // An alias as a mapping key is beyond this composer.
                    return None;
                }
                self.rest_clean()?;
                Some(n)
            }
            b'|' | b'>' => self.block_scalar(parent),
            // A complex key, an empty key, a plain scalar starting with `?`/`:`, reserved indicators.
            b'?' | b':' | b',' | b']' | b'}' | b'%' | b'@' | b'`' | b'#' => None,
            _ => self.scalar_or_map(c, parent, value_inline),
        }
    }

    /// `&name` / `!tag` (at most one each, in any order), the cursor after the blanks that follow them.
    fn props(&mut self) -> Option<Props<'s>> {
        let mut props = Props::default();
        loop {
            match self.peek() {
                Some(b'&') if props.anchor.is_none() => {
                    self.bump();
                    let from = self.pos;
                    while self.peek().is_some_and(anchor_char) {
                        self.bump();
                    }
                    let name = self.s.get(from..self.pos)?;
                    if name.is_empty() || !(blank(self.peek()) || flow_indicator(self.peek())) {
                        return None;
                    }
                    props.anchor = Some(name);
                }
                Some(b'!') if props.tag.is_none() => {
                    let from = self.pos;
                    while !blank(self.peek()) && !flow_indicator(self.peek()) {
                        self.bump();
                    }
                    props.tag = Some(self.s.get(from..self.pos)?);
                }
                Some(b'&' | b'!') => return None,
                _ => return Some(props),
            }
            while matches!(self.peek(), Some(b' ' | b'\t')) {
                self.bump();
            }
        }
    }

    /// Applies a tag to a composed node.
    fn tagged(tag: Option<&[u8]>, n: Node) -> Option<Node> {
        match tag {
            None => Some(n),
            Some(b"!!str") => match n {
                Node::Scalar(s, _) => Some(Node::Scalar(s, false)),
                Node::Null => Some(Node::Scalar(String::new(), false)),
                Node::Opaque => Some(Node::Opaque),
                Node::Seq(_) | Node::Map(_) => None,
            },
            Some(b"!!seq") => matches!(n, Node::Seq(_)).then_some(n),
            Some(b"!!map") => matches!(n, Node::Map(_)).then_some(n),
            // A scalar tag constructs only from a value its resolver accepts; the result is never a `str`.
            Some(t @ (b"!!int" | b"!!float" | b"!!bool" | b"!!null")) => {
                let text = match &n {
                    Node::Scalar(s, _) => s.as_str(),
                    Node::Null => "",
                    Node::Opaque | Node::Seq(_) | Node::Map(_) => return None,
                };
                let decimal = |t: &str| {
                    let d = t.trim_start_matches(['-', '+']);
                    d.len().saturating_add(1) >= t.len()
                        && d.bytes().any(|c| c.is_ascii_digit())
                        && d.bytes().all(|c| c.is_ascii_digit() || c == b'_')
                };
                // `construct_yaml_null` ignores the value; the others parse it.
                let ok = match t {
                    b"!!int" => is_int(text),
                    b"!!float" => is_float(text) || decimal(text),
                    b"!!bool" => matches!(
                        text.to_ascii_lowercase().as_str(),
                        "yes" | "no" | "true" | "false" | "on" | "off"
                    ),
                    _ => true,
                };
                ok.then_some(Node::Null)
            }
            Some(_) => None,
        }
    }

    /// Records an anchored node.
    fn anchor(&mut self, name: Option<&'s [u8]>, n: &Node) -> Option<()> {
        let Some(name) = name else {
            return Some(());
        };
        if self.anchors.len() >= MAX_ANCHORS || self.anchors.iter().any(|(a, _, _)| *a == name) {
            // "found duplicate anchor" (or more anchors than this composer keeps).
            return None;
        }
        let (height, _) = measure(n);
        self.anchors.push((name, n.clone(), height));
        Some(())
    }

    /// `*name`: a copy of the anchored node (bounded in depth and in total size).
    fn alias(&mut self) -> Option<Node> {
        self.bump();
        let from = self.pos;
        while self.peek().is_some_and(anchor_char) {
            self.bump();
        }
        let name = self.s.get(from..self.pos)?;
        if name.is_empty() || !(blank(self.peek()) || flow_indicator(self.peek())) {
            return None;
        }
        let (_, n, height) = self.anchors.iter().find(|(a, _, _)| *a == name)?;
        if self.depth.saturating_add(*height) > MAX_DEPTH {
            return None;
        }
        let (_, size) = measure(n);
        self.copied = self.copied.saturating_add(size);
        if self.copied > MAX_ALIAS_NODES {
            return None;
        }
        Some(n.clone())
    }

    /// A node after its properties; `c` is the column of the first property (the mapping column when they
    /// precede a key).
    fn with_properties(
        &mut self,
        c: i64,
        parent: i64,
        indentless: bool,
        value_inline: bool,
    ) -> Option<Node> {
        let props = self.props()?;
        if !self.token_on_this_line()? {
            let n = Self::tagged(props.tag, self.node(parent, indentless, false)?)?;
            self.anchor(props.anchor, &n)?;
            return Some(n);
        }
        // The node is on this line.
        let n = match self.peek()? {
            b'[' | b'{' => {
                let n = self.flow()?;
                self.rest_clean()?;
                n
            }
            b'|' | b'>' => self.block_scalar(parent)?,
            b'&' | b'!' | b'*' | b'-' | b'?' | b':' | b',' | b']' | b'}' | b'%' | b'@' | b'`'
            | b'#' => return None,
            _ => {
                let key_start = self.pos;
                let (text, plain, at_colon) = self.scalar_line(false)?;
                if at_colon || self.value_indicator() {
                    // The properties belong to the first key of a block mapping at their column.
                    if value_inline || props.tag.is_some() {
                        return None;
                    }
                    if self.pos.saturating_sub(key_start) > MAX_SIMPLE_KEY {
                        return None;
                    }
                    let key = Node::Scalar(text, plain);
                    self.anchor(props.anchor, &key)?;
                    return self.block_map(c, key);
                }
                self.scalar_end(text, plain, parent)?
            }
        };
        let n = Self::tagged(props.tag, n)?;
        self.anchor(props.anchor, &n)?;
        Some(n)
    }

    /// A block sequence at column `c` (`indentless`: it ends at a non-`-` token of the same column).
    fn block_seq(&mut self, c: i64, indentless: bool) -> Option<Node> {
        self.enter()?;
        let mut items = Vec::new();
        loop {
            // At a `-` of column `c`.
            self.bump();
            if self.peek() == Some(b'\t') {
                // libyaml rejects a tab right after the indicator.
                return None;
            }
            items.push(self.node(c, false, false)?);
            self.skip_to_token()?;
            if self.peek().is_none() || self.marker_at(self.pos).is_some() {
                break;
            }
            let col = self.col(self.pos);
            if col < c {
                break;
            }
            if col > c {
                return None;
            }
            let dash = self.peek() == Some(b'-') && blank(self.peek_at(1));
            if !dash {
                if indentless {
                    break;
                }
                return None;
            }
        }
        self.leave();
        Some(Node::Seq(items))
    }

    /// A scalar at column `c`, or the block mapping it is the first key of.
    fn scalar_or_map(&mut self, c: i64, parent: i64, value_inline: bool) -> Option<Node> {
        let key_start = self.pos;
        let (text, plain, at_colon) = self.scalar_line(false)?;
        if at_colon || self.value_indicator() {
            if value_inline {
                // `key: a: b` / `key: 'a': b`: "mapping values are not allowed here".
                return None;
            }
            if self.pos.saturating_sub(key_start) > MAX_SIMPLE_KEY {
                return None;
            }
            return self.block_map(c, Node::Scalar(text, plain));
        }
        self.scalar_end(text, plain, parent)
    }

    /// The end of a block scalar node: a plain scalar's continuation lines, a quoted one's clean line end.
    fn scalar_end(&mut self, text: String, plain: bool, parent: i64) -> Option<Node> {
        if plain {
            let text = self.plain_continuation(text, parent)?;
            return Some(Node::Scalar(text, true));
        }
        self.rest_clean()?;
        Some(Node::Scalar(text, false))
    }

    /// Blanks then a `:` followed by a blank (the cursor moves to the `:`, which stays unconsumed).
    fn value_indicator(&mut self) -> bool {
        let mut p = self.pos;
        while matches!(self.at(p), Some(b' ' | b'\t')) {
            p = p.saturating_add(1);
        }
        if self.at(p) == Some(b':') && blank(self.at(p.saturating_add(1))) {
            self.pos = p;
            return true;
        }
        false
    }

    /// A block mapping at column `c` whose first key was read (the cursor is at its `:`).
    fn block_map(&mut self, c: i64, first: Node) -> Option<Node> {
        self.enter()?;
        let mut entries = Vec::new();
        let mut key = first;
        loop {
            if !key.hashable() {
                return None;
            }
            // At the `:`.
            self.bump();
            let value = self.node(c, true, true)?;
            entries.push((key, value));
            self.skip_to_token()?;
            if self.peek().is_none() || self.marker_at(self.pos).is_some() {
                break;
            }
            let col = self.col(self.pos);
            if col < c {
                break;
            }
            if col > c {
                return None;
            }
            match self.peek()? {
                b'-' if blank(self.peek_at(1)) => return None,
                b'[' | b'{' | b'&' | b'!' | b'*' | b'?' | b':' | b',' | b']' | b'}' | b'|'
                | b'>' | b'%' | b'@' | b'`' | b'#' => return None,
                _ => {}
            }
            let key_start = self.pos;
            let (text, plain, at_colon) = self.scalar_line(false)?;
            if !(at_colon || self.value_indicator())
                || self.pos.saturating_sub(key_start) > MAX_SIMPLE_KEY
            {
                return None;
            }
            key = Node::Scalar(text, plain);
        }
        self.leave();
        Some(Node::Map(entries))
    }

    /// A one-line scalar at the cursor: `(value, plain, stopped at a value indicator)`. A plain scalar (whose first
    /// character the caller vetted) stops at `: `, ` #` or the end of the line, in `flow` context also at `,[]{}`
    /// and at a `:` before a flow indicator.
    fn scalar_line(&mut self, flow: bool) -> Option<(String, bool, bool)> {
        match self.peek()? {
            b'\'' => self.single_quoted().map(|s| (s, false, false)),
            b'"' => self.double_quoted().map(|s| (s, false, false)),
            _ => {
                let from = self.pos;
                self.bump();
                let mut at_colon = false;
                loop {
                    match self.peek() {
                        None | Some(b'\n') => break,
                        Some(b':')
                            if blank(self.peek_at(1))
                                || (flow && flow_indicator(self.peek_at(1))) =>
                        {
                            at_colon = true;
                            break;
                        }
                        Some(b'#')
                            if self
                                .pos
                                .checked_sub(1)
                                .and_then(|p| self.at(p))
                                .is_some_and(|p| p == b' ' || p == b'\t') =>
                        {
                            break;
                        }
                        Some(b',' | b'[' | b']' | b'{' | b'}') if flow => break,
                        Some(_) => self.bump(),
                    }
                }
                let raw = std::str::from_utf8(self.s.get(from..self.pos)?).ok()?;
                let text = raw.trim_end_matches([' ', '\t']);
                if text.is_empty() {
                    return None;
                }
                Some((text.to_owned(), true, at_colon))
            }
        }
    }

    /// The continuation lines of a block plain scalar more indented than `parent` (folded with spaces; empty lines
    /// fold to `\n`).
    fn plain_continuation(&mut self, mut text: String, parent: i64) -> Option<String> {
        loop {
            let mut p = self.pos;
            while matches!(self.at(p), Some(b' ' | b'\t')) {
                p = p.saturating_add(1);
            }
            if self.at(p) != Some(b'\n') {
                // The end of input, or a comment (which ends a plain scalar).
                return Some(text);
            }
            let mut breaks = 0usize;
            let mut q = p;
            loop {
                if self.at(q) == Some(b'\n') {
                    breaks = breaks.saturating_add(1);
                    q = q.saturating_add(1);
                    continue;
                }
                let mut r = q;
                while self.at(r) == Some(b' ') {
                    r = r.saturating_add(1);
                }
                if self.at(r) == Some(b'\t') {
                    // A tab in the indentation (or on a blank line).
                    return None;
                }
                if self.at(r) == Some(b'\n') {
                    q = r;
                    continue;
                }
                q = r;
                break;
            }
            if self.at(q).is_none()
                || self.at(q) == Some(b'#')
                || self.marker_at(self.line_start(q)).is_some()
                || self.col(q) <= parent
            {
                return Some(text);
            }
            self.pos = q;
            let (more, _, at_colon) = self.scalar_line(false)?;
            if at_colon || self.value_indicator() {
                // A multi-line plain scalar cannot be a key: "mapping values are not allowed here".
                return None;
            }
            if breaks > 1 {
                for _ in 1..breaks {
                    text.push('\n');
                }
            } else {
                text.push(' ');
            }
            text.push_str(&more);
        }
    }

    /// `'…'` on one line (`''` is a quote).
    fn single_quoted(&mut self) -> Option<String> {
        self.bump();
        let mut out = Vec::new();
        loop {
            match self.peek()? {
                b'\n' => return None,
                b'\'' if self.peek_at(1) == Some(b'\'') => {
                    out.push(b'\'');
                    self.pos = self.pos.saturating_add(2);
                }
                b'\'' => {
                    self.bump();
                    return String::from_utf8(out).ok();
                }
                b => {
                    out.push(b);
                    self.bump();
                }
            }
        }
    }

    /// `"…"` on one line with libyaml's escapes.
    fn double_quoted(&mut self) -> Option<String> {
        self.bump();
        let mut out = String::new();
        loop {
            match self.peek()? {
                b'\n' => return None,
                b'"' => {
                    self.bump();
                    return Some(out);
                }
                b'\\' => {
                    let e = self.peek_at(1)?;
                    self.pos = self.pos.saturating_add(2);
                    let c = match e {
                        b'\\' => '\\',
                        b'"' => '"',
                        b'/' => '/',
                        b' ' => ' ',
                        b'0' => '\0',
                        b'a' => '\u{7}',
                        b'b' => '\u{8}',
                        b't' | b'\t' => '\t',
                        b'n' => '\n',
                        b'v' => '\u{b}',
                        b'f' => '\u{c}',
                        b'r' => '\r',
                        b'e' => '\u{1b}',
                        b'N' => '\u{85}',
                        b'_' => '\u{a0}',
                        b'L' => '\u{2028}',
                        b'P' => '\u{2029}',
                        b'x' => self.hex_escape(2)?,
                        b'u' => self.hex_escape(4)?,
                        b'U' => self.hex_escape(8)?,
                        _ => return None,
                    };
                    out.push(c);
                }
                _ => {
                    let rest = std::str::from_utf8(self.s.get(self.pos..)?).ok()?;
                    let c = rest.chars().next()?;
                    out.push(c);
                    self.pos = self.pos.saturating_add(c.len_utf8());
                }
            }
        }
    }

    /// `n` hex digits of an escape.
    fn hex_escape(&mut self, n: usize) -> Option<char> {
        let digits = std::str::from_utf8(self.s.get(self.pos..self.pos.saturating_add(n))?).ok()?;
        if digits.len() != n || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.pos = self.pos.saturating_add(n);
        char::from_u32(u32::from_str_radix(digits, 16).ok()?)
    }

    /// A block scalar (`|` / `>` with optional indicators): its header and every line that belongs to it. The value
    /// is computed for a single content line (the only shape a stage name could take), otherwise it is opaque.
    fn block_scalar(&mut self, parent: i64) -> Option<Node> {
        self.bump();
        let mut explicit: Option<i64> = None;
        let mut chomp: Option<u8> = None;
        for _ in 0..2 {
            match self.peek() {
                Some(c @ (b'+' | b'-')) if chomp.is_none() => {
                    chomp = Some(c);
                    self.bump();
                }
                Some(d @ b'1'..=b'9') if explicit.is_none() => {
                    explicit = Some(i64::from(d.saturating_sub(b'0')));
                    self.bump();
                }
                _ => {}
            }
        }
        if !blank(self.peek()) {
            return None;
        }
        self.rest_clean()?;
        let base = parent.max(0);
        let mut indent: Option<i64> = explicit.map(|e| base.saturating_add(e));
        let mut p = self.line_end(self.pos);
        // (first content line, number of content lines, empty lines before the first, trailing empty lines)
        let mut first: Option<(usize, usize)> = None;
        let (mut content, mut leading, mut trailing, mut odd) = (0usize, 0usize, 0usize, false);
        while self.at(p) == Some(b'\n') {
            let line = p.saturating_add(1);
            let mut q = line;
            while self.at(q) == Some(b' ') {
                q = q.saturating_add(1);
            }
            let spaces = i64::try_from(q.saturating_sub(line)).unwrap_or(i64::MAX);
            let end = self.line_end(line);
            if matches!(self.at(q), None | Some(b'\n')) {
                if content == 0 {
                    leading = leading.saturating_add(1);
                } else {
                    trailing = trailing.saturating_add(1);
                }
                if indent.is_some_and(|i| spaces > i) {
                    odd = true;
                }
            } else {
                let need = *indent.get_or_insert(spaces);
                if spaces < need || spaces <= parent {
                    break;
                }
                content = content.saturating_add(1);
                if trailing > 0 {
                    odd = true;
                }
                let from = line.saturating_add(usize::try_from(need).unwrap_or(usize::MAX));
                first.get_or_insert((from, end));
            }
            p = end;
        }
        self.pos = p;
        let value = match first {
            Some((from, end)) if content == 1 && leading == 0 && !odd => {
                let mut v = std::str::from_utf8(self.s.get(from..end)?).ok()?.to_owned();
                match chomp {
                    Some(b'-') => {}
                    Some(_) => {
                        for _ in 0..=trailing {
                            v.push('\n');
                        }
                    }
                    None => v.push('\n'),
                }
                v
            }
            _ => return Some(Node::Opaque),
        };
        Some(Node::Scalar(value, false))
    }

    /// A flow node (the cursor is at or before its first character).
    fn flow(&mut self) -> Option<Node> {
        self.flow_ws()?;
        match self.peek()? {
            b'[' => {
                self.enter()?;
                self.bump();
                let mut items = Vec::new();
                loop {
                    self.flow_ws()?;
                    if self.peek()? == b']' {
                        self.bump();
                        break;
                    }
                    let item = self.flow()?;
                    self.flow_ws()?;
                    let item = if self.peek()? == b':'
                        && (blank(self.peek_at(1)) || flow_indicator(self.peek_at(1)))
                    {
                        // `[key: value]`: a single-pair mapping.
                        if !item.hashable() {
                            return None;
                        }
                        self.bump();
                        self.flow_ws()?;
                        let value = if matches!(self.peek()?, b',' | b']') {
                            Node::Null
                        } else {
                            self.flow()?
                        };
                        self.flow_ws()?;
                        Node::Map(vec![(item, value)])
                    } else {
                        item
                    };
                    items.push(item);
                    match self.peek()? {
                        b',' => self.bump(),
                        b']' => {
                            self.bump();
                            break;
                        }
                        _ => return None,
                    }
                }
                self.leave();
                Some(Node::Seq(items))
            }
            b'{' => {
                self.enter()?;
                self.bump();
                let mut entries = Vec::new();
                loop {
                    self.flow_ws()?;
                    if self.peek()? == b'}' {
                        self.bump();
                        break;
                    }
                    let key = self.flow()?;
                    if !key.hashable() {
                        return None;
                    }
                    self.flow_ws()?;
                    let value = if self.peek()? == b':' {
                        if !(blank(self.peek_at(1)) || flow_indicator(self.peek_at(1))) {
                            return None;
                        }
                        self.bump();
                        self.flow_ws()?;
                        if matches!(self.peek()?, b',' | b'}') {
                            Node::Null
                        } else {
                            self.flow()?
                        }
                    } else {
                        Node::Null
                    };
                    entries.push((key, value));
                    self.flow_ws()?;
                    match self.peek()? {
                        b',' => self.bump(),
                        b'}' => {
                            self.bump();
                            break;
                        }
                        _ => return None,
                    }
                }
                self.leave();
                Some(Node::Map(entries))
            }
            b'&' | b'!' => {
                self.enter()?;
                let props = self.props()?;
                self.flow_ws()?;
                let n = if matches!(self.peek()?, b',' | b']' | b'}' | b':') {
                    Node::Null
                } else {
                    self.flow()?
                };
                self.leave();
                let n = Self::tagged(props.tag, n)?;
                self.anchor(props.anchor, &n)?;
                Some(n)
            }
            b'*' => self.alias(),
            b'-' if blank(self.peek_at(1)) => None,
            b'?' | b':' | b',' | b']' | b'}' | b'|' | b'>' | b'%' | b'@' | b'`' | b'#' => None,
            _ => {
                let (text, plain, _) = self.scalar_line(true)?;
                if plain && matches!(self.peek(), Some(b'\n')) {
                    // A plain scalar may continue on the next line; only its end there is followed.
                    let mut probe = self.probe();
                    probe.flow_ws()?;
                    if !matches!(probe.peek(), Some(b',' | b']' | b'}' | b':')) {
                        return None;
                    }
                }
                Some(Node::Scalar(text, plain))
            }
        }
    }

    /// Whitespace, line breaks and comments inside a flow collection; a document marker there is an error.
    fn flow_ws(&mut self) -> Option<()> {
        self.skip_to_token()?;
        self.marker_at(self.pos).is_none().then_some(())
    }
}

/// Whether a plain scalar constructs to `str` under PyYAML's implicit resolvers (`resolver.py`).
fn is_plain_str(s: &str) -> bool {
    !(is_bool(s)
        || is_null(s)
        || is_int(s)
        || is_float(s)
        || is_timestamp(s)
        || s == "<<"
        || s == "=")
}

/// `tag:yaml.org,2002:bool`.
fn is_bool(s: &str) -> bool {
    matches!(
        s,
        "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    )
}

/// `tag:yaml.org,2002:null`.
fn is_null(s: &str) -> bool {
    matches!(s, "" | "~" | "null" | "Null" | "NULL")
}

/// A tiny cursor for the resolver patterns.
struct Pat<'a>(&'a [u8]);

impl Pat<'_> {
    fn eat(&mut self, f: impl Fn(u8) -> bool) -> bool {
        match self.0.split_first() {
            Some((&b, rest)) if f(b) => {
                self.0 = rest;
                true
            }
            _ => false,
        }
    }
    fn eat_str(&mut self, lit: &[u8]) -> bool {
        match self.0.strip_prefix(lit) {
            Some(rest) => {
                self.0 = rest;
                true
            }
            None => false,
        }
    }
    /// Zero or more.
    fn star(&mut self, f: impl Fn(u8) -> bool) -> usize {
        let mut n = 0usize;
        while self.eat(&f) {
            n = n.saturating_add(1);
        }
        n
    }
    fn sign(&mut self) {
        let _signed: bool = self.eat(|b| b == b'-' || b == b'+');
    }
    const fn done(&self) -> bool {
        self.0.is_empty()
    }
    /// `(?::[0-5]?[0-9])+`.
    fn sexagesimal(&mut self) -> bool {
        let mut n = 0usize;
        loop {
            let save = self.0;
            if !self.eat(|b| b == b':') {
                break;
            }
            let two =
                matches!(self.0, [a, b, ..] if (b'0'..=b'5').contains(a) && b.is_ascii_digit());
            if two {
                self.0 = self.0.get(2..).unwrap_or_default();
            } else if !self.eat(|b| b.is_ascii_digit()) {
                self.0 = save;
                break;
            }
            n = n.saturating_add(1);
        }
        n > 0
    }
    /// `(?:[eE][-+][0-9]+)?`.
    fn exponent(&mut self) {
        let save = self.0;
        if self.eat(|b| b == b'e' || b == b'E')
            && self.eat(|b| b == b'-' || b == b'+')
            && self.star(|b| b.is_ascii_digit()) > 0
        {
            return;
        }
        self.0 = save;
    }
}

const fn digit_(b: u8) -> bool {
    b.is_ascii_digit() || b == b'_'
}

/// `tag:yaml.org,2002:int`.
fn is_int(s: &str) -> bool {
    let b = s.as_bytes();
    let alts: [&dyn Fn(&mut Pat<'_>) -> bool; 5] = [
        &|p| {
            p.sign();
            p.eat_str(b"0b") && p.star(|c| matches!(c, b'0' | b'1' | b'_')) > 0 && p.done()
        },
        &|p| {
            p.sign();
            p.eat_str(b"0") && p.star(|c| matches!(c, b'0'..=b'7' | b'_')) > 0 && p.done()
        },
        &|p| {
            p.sign();
            if p.eat_str(b"0") {
                return p.done();
            }
            p.eat(|c| matches!(c, b'1'..=b'9')) && {
                p.star(digit_);
                p.done()
            }
        },
        &|p| {
            p.sign();
            p.eat_str(b"0x") && p.star(|c| c.is_ascii_hexdigit() || c == b'_') > 0 && p.done()
        },
        &|p| {
            p.sign();
            p.eat(|c| matches!(c, b'1'..=b'9')) && {
                p.star(digit_);
                p.sexagesimal() && p.done()
            }
        },
    ];
    alts.iter().any(|f| f(&mut Pat(b)))
}

/// `tag:yaml.org,2002:float`.
fn is_float(s: &str) -> bool {
    let b = s.as_bytes();
    let alts: [&dyn Fn(&mut Pat<'_>) -> bool; 5] = [
        &|p| {
            p.sign();
            p.eat(|c| c.is_ascii_digit()) && {
                p.star(digit_);
                p.eat(|c| c == b'.') && {
                    p.star(digit_);
                    p.exponent();
                    p.done()
                }
            }
        },
        &|p| {
            p.eat(|c| c == b'.') && p.eat(|c| c.is_ascii_digit()) && {
                p.star(digit_);
                p.exponent();
                p.done()
            }
        },
        &|p| {
            p.sign();
            p.eat(|c| c.is_ascii_digit()) && {
                p.star(digit_);
                p.sexagesimal() && p.eat(|c| c == b'.') && {
                    p.star(digit_);
                    p.done()
                }
            }
        },
        &|p| {
            p.sign();
            p.eat(|c| c == b'.')
                && (p.eat_str(b"inf") || p.eat_str(b"Inf") || p.eat_str(b"INF"))
                && p.done()
        },
        &|p| {
            p.eat(|c| c == b'.')
                && (p.eat_str(b"nan") || p.eat_str(b"NaN") || p.eat_str(b"NAN"))
                && p.done()
        },
    ];
    alts.iter().any(|f| f(&mut Pat(b)))
}

/// `tag:yaml.org,2002:timestamp`.
fn is_timestamp(s: &str) -> bool {
    let d = |b: u8| b.is_ascii_digit();
    let mut p = Pat(s.as_bytes());
    let mut q = Pat(s.as_bytes());
    let date_only = (0..4).all(|_| p.eat(d))
        && p.eat(|b| b == b'-')
        && p.eat(d)
        && p.eat(d)
        && p.eat(|b| b == b'-')
        && p.eat(d)
        && p.eat(d)
        && p.done();
    if date_only {
        return true;
    }
    let p = &mut q;
    let one_or_two = |p: &mut Pat<'_>| {
        let ok = p.eat(d);
        let _second: bool = p.eat(d);
        ok
    };
    if !((0..4).all(|_| p.eat(d))
        && p.eat(|b| b == b'-')
        && one_or_two(p)
        && p.eat(|b| b == b'-')
        && one_or_two(p))
    {
        return false;
    }
    if !(p.eat(|b| b == b'T' || b == b't') || p.star(|b| b == b' ' || b == b'\t') > 0) {
        return false;
    }
    if !(one_or_two(p)
        && p.eat(|b| b == b':')
        && p.eat(d)
        && p.eat(d)
        && p.eat(|b| b == b':')
        && p.eat(d)
        && p.eat(d))
    {
        return false;
    }
    if p.eat(|b| b == b'.') {
        p.star(d);
    }
    if p.done() {
        return true;
    }
    p.star(|b| b == b' ' || b == b'\t');
    if p.eat(|b| b == b'Z') {
        return p.done();
    }
    if !(p.eat(|b| b == b'-' || b == b'+') && one_or_two(p)) {
        return false;
    }
    if p.eat(|b| b == b':') && !(p.eat(d) && p.eat(d)) {
        return false;
    }
    p.done()
}
#[cfg(test)]
mod tests {
    use super::*;

    /// No legacy stage.
    const NONE: [&str; 0] = [];

    #[test]
    fn block_and_flow_sequences_in_first_seen_order() {
        let m = b"-   id: a\n    stages: [push, manual]\n-   id: b\n    stages:\n    - commit\n    -   'merge-commit'\n    - push\n-   id: c\n    entry: c\n";
        assert_eq!(legacy_stages(m), ["push", "commit", "merge-commit"]);
    }

    #[test]
    fn new_names_and_non_stage_keys_warn_nothing() {
        let m = b"-   id: a\n    stages: [pre-commit, pre-push]\n    name: commit push\n    args: [commit]\n";
        assert_eq!(legacy_stages(m), NONE);
    }

    #[test]
    fn invalid_minimal_manifest_warns_nothing() {
        // pre-commit swallows the InvalidManifestError of the minimal schema.
        assert_eq!(legacy_stages(b"-   id: a\n    stages: commit\n"), NONE);
        assert_eq!(legacy_stages(b"-   id: a\n    stages: [commit, 1]\n"), NONE);
        assert_eq!(
            legacy_stages(b"-   id: a\n    stages: [commit, true]\n"),
            NONE
        );
        assert_eq!(
            legacy_stages(b"-   id: a\n    stages:\n-   id: b\n    stages: [commit]\n"),
            NONE
        );
        assert_eq!(
            legacy_stages(b"\xff-   id: a\n    stages: [commit]\n"),
            NONE
        );
    }

    #[test]
    fn first_key_on_the_dash_line_and_multiline_flow() {
        let m = b"- stages: [\n    commit,  # old\n    push\n  ]\n  id: a\n";
        assert_eq!(legacy_stages(m), ["commit", "push"]);
        let m = b"-   stages:\n    - merge-commit\n    id: a\n";
        assert_eq!(legacy_stages(m), ["merge-commit"]);
    }

    #[test]
    fn quoted_keys_and_comments() {
        let m = b"-   id: a\n    'stages': [\"commit\"] # c\n    # stages: [push]\n";
        assert_eq!(legacy_stages(m), ["commit"]);
    }

    #[test]
    fn embedded_pre_v5_manifests_use_legacy_stages() {
        let p = snp_hooks_data::profile_for_rev("v4.4.0").unwrap();
        assert_eq!(
            legacy_stages(p.manifest_yaml.as_bytes()),
            ["commit", "push"]
        );
        let p = snp_hooks_data::profile_for_rev("v6.0.0").unwrap();
        assert_eq!(legacy_stages(p.manifest_yaml.as_bytes()), NONE);
    }

    #[test]
    fn pyyaml_implicit_resolvers() {
        for s in [
            "1",
            "-2",
            "+0",
            "0b10",
            "017",
            "0x1f",
            "1_000",
            "1:30",
            "1.5",
            "-1.",
            ".5",
            "1.0e+3",
            "1:30.5",
            ".inf",
            "-.Inf",
            ".NaN",
            "null",
            "~",
            "",
            "Yes",
            "off",
            "TRUE",
            "2001-12-14",
            "2001-12-14t21:59:43.10-05:00",
            "2001-12-14 21:59:43.10",
            "<<",
            "=",
        ] {
            assert!(!is_plain_str(s), "{s}");
        }
        for s in [
            "commit",
            "pre-commit",
            "y",
            "n",
            "e",
            "-",
            "1a",
            "1e3",
            "09",
            "0o7",
            ".",
            "1.0e3",
            "2001-1-1",
            "yes!",
            "nil",
            "-.inf.",
            "1:60",
        ] {
            assert!(is_plain_str(s), "{s}");
        }
    }

    /// The review's minimal pairs: pre-commit warns on each of these (the line scanner did not).
    #[test]
    fn markers_directives_flow_mappings_anchors_tags_and_line_breaks() {
        for m in [
            &b"---\n-   id: a\n    stages: [commit]\n"[..],
            b"-\n    id: a\n    stages: [commit]\n",
            b"-   id: a\n    stages: [commit]\n...\n",
            b"-   id: a\r    stages: [commit]\r",
            b"-   id: a\r\n    stages: [commit]\r\n",
            b"- {id: a, stages: [commit]}\n",
            b"-   id: &x a\n    stages: [commit]\n",
            b"-   id: a\n    stages: !!seq [commit]\n",
            b"%YAML 1.1\n---\n-   id: a\n    stages: [commit]\n",
            b"- &h\n  id: a\n  stages: [commit]\n- *h\n",
        ] {
            assert_eq!(
                legacy_stages(m),
                ["commit"],
                "{:?}",
                String::from_utf8_lossy(m)
            );
        }
    }

    /// The review's false positives: PyYAML rejects these, so pre-commit prints nothing.
    #[test]
    fn mapping_values_and_nested_sequences_where_a_scalar_was_expected() {
        assert_eq!(legacy_stages(b"-   id: a:\n    stages: [commit]\n"), NONE);
        assert_eq!(
            legacy_stages(b"-   id: a\n    stages:\n    - commit\n    - -\n"),
            NONE
        );
        assert_eq!(
            legacy_stages(b"-   id: a\n    stages: [commit]\n-   id: &x b\n-   id: &x c\n"),
            NONE
        );
    }

    /// pre-commit 4.7.0's own verdict on hand-written edge cases and on every upstream tag's manifest.
    #[test]
    fn oracle_vectors() {
        let tsv = include_str!("../tests/vectors/legacy_stages.tsv");
        let mut n = 0;
        let mut bad = Vec::new();
        for line in tsv.lines().filter(|l| !l.starts_with('#')) {
            let (hex, want) = line.split_once('\t').unwrap();
            let nibble = |c: u8| char::from(c).to_digit(16).unwrap();
            let mut bytes = Vec::new();
            let mut it = hex.bytes();
            while let (Some(h), Some(l)) = (it.next(), it.next()) {
                bytes.push(u8::try_from(nibble(h) * 16 + nibble(l)).unwrap());
            }
            let want: Vec<&str> = want.split(',').filter(|s| !s.is_empty()).collect();
            if legacy_stages(&bytes) != want {
                bad.push(format!(
                    "{:?}: snp {:?}, pre-commit {want:?}",
                    String::from_utf8_lossy(&bytes),
                    legacy_stages(&bytes)
                ));
            }
            n += 1;
        }
        assert!(n > 40, "{n}");
        assert_eq!(bad, Vec::<String>::new());
    }

    /// pre-commit 4.7.0's verdict on a generated differential corpus (`gen_legacy_stages_fuzz.py`): never a
    /// warning pre-commit does not print; a missed one only behind a construct this composer does not follow
    /// (a `?` complex key).
    #[test]
    fn oracle_fuzz_corpus() {
        let tsv = include_str!("../tests/vectors/legacy_stages_fuzz.tsv");
        let (mut n, mut warned) = (0, 0);
        let mut bad = Vec::new();
        for line in tsv.lines().filter(|l| !l.starts_with('#')) {
            let (hex, want) = line.split_once('\t').unwrap();
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2).unwrap(), 16).unwrap())
                .collect();
            let want: Vec<&str> = want.split(',').filter(|s| !s.is_empty()).collect();
            let got = legacy_stages(&bytes);
            let text = String::from_utf8_lossy(&bytes);
            let excused = got.is_empty() && text.contains("? ");
            if got != want && !excused {
                bad.push(format!("{text:?}: snp {got:?}, pre-commit {want:?}"));
            }
            warned += usize::from(!want.is_empty());
            n += 1;
        }
        assert!(n > 1000 && warned > 500, "{n} {warned}");
        assert_eq!(bad, Vec::<String>::new());
    }

    /// Hostile shapes stay bounded (nesting, aliases, long lines) and decline instead of recursing or copying.
    #[test]
    fn hostile_inputs_are_bounded() {
        let deep = format!(
            "- id: a\n  stages: [commit]\n  x: {}{}\n",
            std::iter::repeat_n("[", 10_000).collect::<String>(),
            std::iter::repeat_n("]", 10_000).collect::<String>()
        );
        assert_eq!(legacy_stages(deep.as_bytes()), NONE);
        let deep = format!(
            "- id: a\n  stages: [commit]\n  x:\n  {}a\n",
            std::iter::repeat_n("- ", 200).collect::<String>()
        );
        assert_eq!(legacy_stages(deep.as_bytes()), NONE);
        let laughs = format!(
            "- id: a\n  stages: [commit]\n  a: &a [{}]\n  b: &b [{}]\n  c: [{}]\n",
            ["x"; 100].join(","),
            ["*a"; 100].join(","),
            ["*b"; 100].join(",")
        );
        assert_eq!(legacy_stages(laughs.as_bytes()), NONE);
        let long = format!(
            "- id: a\n  stages: [{}commit]\n",
            std::iter::repeat_n("pre-commit,", 200_000).collect::<String>()
        );
        assert_eq!(legacy_stages(long.as_bytes()), ["commit"]);
    }

    /// No input makes the scanner panic: every prefix and every single-byte mutation of the vector manifests.
    #[test]
    fn arbitrary_input_never_panics() {
        let seeds = [
            &b"- stages: [\n    commit,  # old\n    push\n  ]\n  id: a\n"[..],
            b"-   id: a\n    'stages':\n    -   \"commit\"\n    -   push\n",
            b"%YAML 1.1\n--- # c\n- &h {id: a, stages: [!!str commit, *s]}\n- *h\n...\n",
            b"-\n  id: &s a\n  x: |-\n    b\n  y: >\n\n  stages:\n  - \"comm\\x69t\"\n  - [a: b]\n",
        ];
        for seed in seeds {
            for end in 0..=seed.len() {
                std::hint::black_box(legacy_stages(&seed[..end]));
            }
            for i in 0..seed.len() {
                for b in [
                    b'[', b']', b',', b'\'', b'"', b'#', b'\n', b' ', b'-', b':', 0xff, b'\t',
                    b'{', b'}', b'&', b'*', b'!', b'|', b'>', b'?', b'\r', b'\\', b'%',
                ] {
                    let mut m = seed.to_vec();
                    m[i] = b;
                    std::hint::black_box(legacy_stages(&m));
                    let inserted: Vec<u8> = seed[..i]
                        .iter()
                        .chain(std::iter::once(&b))
                        .chain(seed[i..].iter())
                        .copied()
                        .collect();
                    std::hint::black_box(legacy_stages(&inserted));
                }
            }
        }
    }
}
