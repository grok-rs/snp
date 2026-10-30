//! libyaml 0.2.5's emitter (`emitter.c`), the half of `CSafeDumper` that writes text, ported for the event
//! stream PyYAML's `CEmitter.serialize` produces for `SafeRepresenter` nodes: one implicit document, block
//! collections (an empty one is written in flow style, `[]` / `{}`), scalars in any style, `!!` tags, no anchors.
//! Settings are those of `yaml.dump(…, default_flow_style=False, indent=4)`: indent 4, width 80, `allow_unicode`
//! off (non-ASCII is escaped in double quotes), `\n` line breaks, not canonical.
//!
//! The port keeps libyaml's state machine and its whitespace/indention bookkeeping verbatim; it is infallible
//! (the representer only produces well-formed streams) and iterative (deep values cost heap, not stack).
//!
//! Owner: WP-14.

use std::fmt::Write as _;

/// libyaml's `best_indent` for `indent=4`.
const BEST_INDENT: i32 = 4;
/// libyaml's default `best_width`.
const BEST_WIDTH: i32 = 80;

/// The scalar styles PyYAML can request (`node.style`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Plain,
    Single,
    Double,
    Literal,
}

/// One serializer event (`_yaml.pyx` `CEmitter._serialize_node`). Empty collections are single events because
/// libyaml writes them in flow style after looking one event ahead (`check_empty_sequence`). `anchor` is the
/// `idNNN` name of a node that occurs more than once (its first occurrence); later occurrences are [`Event::Alias`].
#[derive(Debug, Clone)]
pub(crate) enum Event {
    Scalar {
        anchor: Option<String>,
        tag: &'static str,
        value: String,
        plain_implicit: bool,
        quoted_implicit: bool,
        style: Style,
    },
    SequenceStart {
        anchor: Option<String>,
        tag: &'static str,
        implicit: bool,
    },
    SequenceEnd,
    MappingStart {
        anchor: Option<String>,
        tag: &'static str,
        implicit: bool,
    },
    MappingEnd,
    EmptySequence {
        anchor: Option<String>,
        tag: &'static str,
        implicit: bool,
    },
    EmptyMapping {
        anchor: Option<String>,
        tag: &'static str,
        implicit: bool,
    },
    Alias(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    DocumentContent,
    DocumentEnd,
    BlockSequenceFirstItem,
    BlockSequenceItem,
    BlockMappingFirstKey,
    BlockMappingKey,
    BlockMappingSimpleValue,
    BlockMappingValue,
}

/// `yaml_emitter_analyze_tag` against the default tag directives `!` → `!` and `!!` → `tag:yaml.org,2002:`.
#[derive(Copy, Clone, Debug)]
struct TagAnalysis<'a> {
    handle: &'a str,
    suffix: &'a str,
}

fn analyze_tag(tag: &str) -> TagAnalysis<'_> {
    for (handle, prefix) in [("!", "!"), ("!!", "tag:yaml.org,2002:")] {
        if prefix.len() < tag.len()
            && let Some(suffix) = tag.strip_prefix(prefix)
        {
            return TagAnalysis { handle, suffix };
        }
    }
    TagAnalysis {
        handle: "",
        suffix: tag,
    }
}

/// `yaml_emitter_analyze_scalar`.
#[derive(Copy, Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "libyaml's scalar analysis is a set of flags"
)]
struct ScalarAnalysis {
    multiline: bool,
    flow_plain_allowed: bool,
    block_plain_allowed: bool,
    single_quoted_allowed: bool,
    block_allowed: bool,
}

/// libyaml's `IS_PRINTABLE` (on the UTF-8 encoding: `#xA`, `#x20-#x7E`, `#xA0-#xD7FF`, `#xE000-#xFFFD` except
/// `#xFEFF`; characters beyond the BMP are *not* printable in libyaml 0.2.5).
const fn is_printable(c: char) -> bool {
    matches!(c, '\n' | ' '..='~' | '\u{a0}'..='\u{d7ff}' | '\u{e000}'..='\u{fefe}' | '\u{ff00}'..='\u{fffd}')
}

/// `IS_BREAK`.
const fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `IS_BLANKZ` (`NUL` and the end of the string count as `Z`).
fn is_blankz(c: Option<char>) -> bool {
    match c {
        None | Some(' ' | '\t' | '\0') => true,
        Some(c) => is_break(c),
    }
}

fn analyze_scalar(value: &str) -> ScalarAnalysis {
    if value.is_empty() {
        return ScalarAnalysis {
            multiline: false,
            flow_plain_allowed: false,
            block_plain_allowed: true,
            single_quoted_allowed: true,
            block_allowed: false,
        };
    }
    let mut block_indicators = value.starts_with("---") || value.starts_with("...");
    let mut flow_indicators = block_indicators;
    let mut line_breaks = false;
    let mut special_characters = false;
    let mut leading_space = false;
    let mut leading_break = false;
    let mut trailing_space = false;
    let mut trailing_break = false;
    let mut break_space = false;
    let mut space_break = false;
    let mut preceded_by_whitespace = true;
    let mut previous_space = false;
    let mut previous_break = false;
    let mut chars = value.chars().peekable();
    let mut first = true;
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let followed_by_whitespace = is_blankz(next);
        if first {
            match c {
                '#' | ',' | '[' | ']' | '{' | '}' | '&' | '*' | '!' | '|' | '>' | '\'' | '"'
                | '%' | '@' | '`' => {
                    flow_indicators = true;
                    block_indicators = true;
                }
                '?' | ':' => {
                    flow_indicators = true;
                    if followed_by_whitespace {
                        block_indicators = true;
                    }
                }
                '-' if followed_by_whitespace => {
                    flow_indicators = true;
                    block_indicators = true;
                }
                _ => {}
            }
        } else {
            match c {
                ',' | '?' | '[' | ']' | '{' | '}' => flow_indicators = true,
                ':' => {
                    flow_indicators = true;
                    if followed_by_whitespace {
                        block_indicators = true;
                    }
                }
                '#' if preceded_by_whitespace => {
                    flow_indicators = true;
                    block_indicators = true;
                }
                _ => {}
            }
        }
        // `allow_unicode` is off: every non-ASCII character is special.
        if !is_printable(c) || !c.is_ascii() {
            special_characters = true;
        }
        if is_break(c) {
            line_breaks = true;
        }
        if c == ' ' {
            if first {
                leading_space = true;
            }
            if next.is_none() {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if is_break(c) {
            if first {
                leading_break = true;
            }
            if next.is_none() {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }
        preceded_by_whitespace = is_blankz(Some(c));
        first = false;
    }
    let mut a = ScalarAnalysis {
        multiline: line_breaks,
        flow_plain_allowed: true,
        block_plain_allowed: true,
        single_quoted_allowed: true,
        block_allowed: true,
    };
    if leading_space || leading_break || trailing_space || trailing_break {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
    }
    if trailing_space {
        a.block_allowed = false;
    }
    if break_space {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
        a.single_quoted_allowed = false;
    }
    if space_break || special_characters {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
        a.single_quoted_allowed = false;
        a.block_allowed = false;
    }
    if line_breaks {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
    }
    if flow_indicators {
        a.flow_plain_allowed = false;
    }
    if block_indicators {
        a.block_plain_allowed = false;
    }
    a
}

/// The emitter; [`Emitter::emit`] the events of one document, then [`Emitter::finish`].
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "libyaml's emitter state is a set of flags"
)]
pub(crate) struct Emitter {
    out: String,
    states: Vec<State>,
    state: State,
    indents: Vec<i32>,
    indent: i32,
    /// Always 0: only empty collections are written in flow style, and they have no content.
    flow_level: i32,
    mapping_context: bool,
    simple_key_context: bool,
    column: i32,
    whitespace: bool,
    indention: bool,
    open_ended: i32,
}

impl Emitter {
    /// STREAM-START and an implicit DOCUMENT-START (`yaml_emitter_emit_stream_start`,
    /// `yaml_emitter_emit_document_start` with no directives: nothing is written).
    pub(crate) fn new() -> Self {
        Self {
            out: String::new(),
            states: Vec::new(),
            state: State::DocumentContent,
            indents: Vec::new(),
            indent: -1,
            flow_level: 0,
            mapping_context: false,
            simple_key_context: false,
            column: 0,
            whitespace: true,
            indention: true,
            open_ended: 0,
        }
    }

    /// An implicit DOCUMENT-END and STREAM-END; the output.
    pub(crate) fn finish(mut self) -> String {
        // yaml_emitter_emit_document_end (implicit)
        self.write_indent();
        if self.open_ended == 0 {
            self.open_ended = 1;
        }
        // yaml_emitter_emit_document_start on STREAM-END
        if self.open_ended == 2 {
            self.write_indicator("...", true, false, false);
            self.open_ended = 0;
            self.write_indent();
        }
        self.out
    }

    /// `yaml_emitter_emit` + `yaml_emitter_state_machine` for one event.
    pub(crate) fn emit(&mut self, ev: &Event) {
        match self.state {
            State::DocumentContent => {
                self.states.push(State::DocumentEnd);
                self.emit_node(ev, false, false);
            }
            State::BlockSequenceFirstItem => self.emit_block_sequence_item(ev, true),
            State::BlockSequenceItem => self.emit_block_sequence_item(ev, false),
            State::BlockMappingFirstKey => self.emit_block_mapping_key(ev, true),
            State::BlockMappingKey => self.emit_block_mapping_key(ev, false),
            State::BlockMappingSimpleValue => self.emit_block_mapping_value(ev, true),
            State::BlockMappingValue => self.emit_block_mapping_value(ev, false),
            // Nothing follows the root node of the single document.
            State::DocumentEnd => {}
        }
    }

    fn pop_state(&mut self) {
        self.state = self.states.pop().unwrap_or(State::DocumentEnd);
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(-1);
    }

    fn increase_indent(&mut self, flow: bool, indentless: bool) {
        self.indents.push(self.indent);
        if self.indent < 0 {
            self.indent = if flow { BEST_INDENT } else { 0 };
        } else if !indentless {
            self.indent = self.indent.saturating_add(BEST_INDENT);
        }
    }

    fn emit_block_sequence_item(&mut self, ev: &Event, first: bool) {
        if first {
            self.increase_indent(false, self.mapping_context && !self.indention);
        }
        if matches!(ev, Event::SequenceEnd) {
            self.pop_indent();
            self.pop_state();
            return;
        }
        self.write_indent();
        self.write_indicator("-", true, false, true);
        self.states.push(State::BlockSequenceItem);
        self.emit_node(ev, false, false);
    }

    fn emit_block_mapping_key(&mut self, ev: &Event, first: bool) {
        if first {
            self.increase_indent(false, false);
        }
        if matches!(ev, Event::MappingEnd) {
            self.pop_indent();
            self.pop_state();
            return;
        }
        self.write_indent();
        if check_simple_key(ev) {
            self.states.push(State::BlockMappingSimpleValue);
            self.emit_node(ev, true, true);
        } else {
            self.write_indicator("?", true, false, true);
            self.states.push(State::BlockMappingValue);
            self.emit_node(ev, true, false);
        }
    }

    fn emit_block_mapping_value(&mut self, ev: &Event, simple: bool) {
        if simple {
            self.write_indicator(":", false, false, false);
        } else {
            self.write_indent();
            self.write_indicator(":", true, false, true);
        }
        self.states.push(State::BlockMappingKey);
        self.emit_node(ev, true, false);
    }

    /// `yaml_emitter_emit_node` (the root and sequence contexts only matter for libyaml's open-ended
    /// documents and canonical output, which `yaml.dump` never uses).
    fn emit_node(&mut self, ev: &Event, mapping: bool, simple_key: bool) {
        self.mapping_context = mapping;
        self.simple_key_context = simple_key;
        match ev {
            Event::Scalar { .. } => self.emit_scalar(ev),
            Event::Alias(name) => {
                // yaml_emitter_emit_alias
                self.process_anchor("*", name);
                if self.simple_key_context {
                    self.put(' ');
                }
                self.pop_state();
            }
            Event::SequenceStart {
                anchor,
                tag,
                implicit,
            } => {
                self.process_collection_properties(anchor.as_deref(), tag, *implicit);
                self.state = State::BlockSequenceFirstItem;
            }
            Event::MappingStart {
                anchor,
                tag,
                implicit,
            } => {
                self.process_collection_properties(anchor.as_deref(), tag, *implicit);
                self.state = State::BlockMappingFirstKey;
            }
            Event::EmptySequence {
                anchor,
                tag,
                implicit,
            } => {
                self.process_collection_properties(anchor.as_deref(), tag, *implicit);
                self.emit_empty_flow("[", "]");
            }
            Event::EmptyMapping {
                anchor,
                tag,
                implicit,
            } => {
                self.process_collection_properties(anchor.as_deref(), tag, *implicit);
                self.emit_empty_flow("{", "}");
            }
            // An end event where a node is expected cannot come from the representer.
            Event::SequenceEnd | Event::MappingEnd => self.pop_state(),
        }
    }

    /// `yaml_emitter_emit_flow_sequence_item` / `…_flow_mapping_key` for a collection whose first event is its
    /// end: the opening indicator, then (the indent and flow level restored at once) the closing one.
    fn emit_empty_flow(&mut self, open: &str, close: &str) {
        self.write_indicator(open, true, true, false);
        self.write_indicator(close, false, false, false);
        self.pop_state();
    }

    /// `yaml_emitter_process_anchor` + `yaml_emitter_process_tag` of a collection start.
    fn process_collection_properties(&mut self, anchor: Option<&str>, tag: &str, implicit: bool) {
        if let Some(name) = anchor {
            self.process_anchor("&", name);
        }
        if !implicit {
            self.process_tag(Some(analyze_tag(tag)));
        }
    }

    /// `yaml_emitter_process_anchor`: `&name` or `*name`.
    fn process_anchor(&mut self, indicator: &str, name: &str) {
        self.write_indicator(indicator, true, false, false);
        // yaml_emitter_write_anchor
        self.write_str(name);
        self.whitespace = false;
        self.indention = false;
    }

    /// `yaml_emitter_emit_scalar`.
    fn emit_scalar(&mut self, ev: &Event) {
        let Event::Scalar {
            anchor,
            tag,
            value,
            plain_implicit,
            quoted_implicit,
            style,
        } = ev
        else {
            return;
        };
        let analysis = analyze_scalar(value);
        let mut tag_analysis = (!*plain_implicit && !*quoted_implicit).then(|| analyze_tag(tag));
        let style = self.select_scalar_style(
            value,
            analysis,
            &mut tag_analysis,
            *style,
            *plain_implicit,
            *quoted_implicit,
        );
        if let Some(name) = anchor {
            self.process_anchor("&", name);
        }
        self.process_tag(tag_analysis);
        self.increase_indent(true, false);
        let allow_breaks = !self.simple_key_context;
        match style {
            Style::Plain => self.write_plain_scalar(value, allow_breaks),
            Style::Single => self.write_single_quoted_scalar(value, allow_breaks),
            Style::Double => self.write_double_quoted_scalar(value, allow_breaks),
            Style::Literal => self.write_literal_scalar(value),
        }
        self.pop_indent();
        self.pop_state();
    }

    /// `yaml_emitter_select_scalar_style`.
    fn select_scalar_style(
        &self,
        value: &str,
        a: ScalarAnalysis,
        tag: &mut Option<TagAnalysis<'_>>,
        requested: Style,
        plain_implicit: bool,
        quoted_implicit: bool,
    ) -> Style {
        let no_tag = tag.is_none();
        let mut style = if self.simple_key_context && a.multiline {
            Style::Double
        } else {
            requested
        };
        if style == Style::Plain {
            if (self.flow_level != 0 && !a.flow_plain_allowed)
                || (self.flow_level == 0 && !a.block_plain_allowed)
            {
                style = Style::Single;
            }
            if value.is_empty() && (self.flow_level != 0 || self.simple_key_context) {
                style = Style::Single;
            }
            if no_tag && !plain_implicit {
                style = Style::Single;
            }
        }
        if style == Style::Single && !a.single_quoted_allowed {
            style = Style::Double;
        }
        if style == Style::Literal
            && (!a.block_allowed || self.flow_level != 0 || self.simple_key_context)
        {
            style = Style::Double;
        }
        if no_tag && !quoted_implicit && style != Style::Plain {
            *tag = Some(TagAnalysis {
                handle: "!",
                suffix: "",
            });
        }
        style
    }

    /// `yaml_emitter_process_tag`.
    fn process_tag(&mut self, tag: Option<TagAnalysis<'_>>) {
        let Some(t) = tag else {
            return;
        };
        if t.handle.is_empty() && t.suffix.is_empty() {
            return;
        }
        if t.handle.is_empty() {
            self.write_indicator("!<", true, false, false);
            self.write_tag_content(t.suffix);
            self.write_indicator(">", false, false, false);
        } else {
            self.write_tag_handle(t.handle);
            if !t.suffix.is_empty() {
                self.write_tag_content(t.suffix);
            }
        }
    }

    // --- writers -------------------------------------------------------------------------------------------------

    fn put(&mut self, c: char) {
        self.out.push(c);
        self.column = self.column.saturating_add(1);
    }

    fn put_break(&mut self) {
        self.out.push('\n');
        self.column = 0;
    }

    fn write_str(&mut self, s: &str) {
        self.out.push_str(s);
        self.column = self
            .column
            .saturating_add(i32::try_from(s.chars().count()).unwrap_or(i32::MAX));
    }

    /// `WRITE_BREAK`: `\n` becomes the line break, other breaks are copied.
    fn write_break(&mut self, c: char) {
        if c == '\n' {
            self.put_break();
        } else {
            self.out.push(c);
            self.column = 0;
        }
    }

    fn write_indent(&mut self) {
        let indent = self.indent.max(0);
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.put_break();
        }
        while self.column < indent {
            self.put(' ');
        }
        self.whitespace = true;
        self.indention = true;
    }

    fn write_indicator(
        &mut self,
        ind: &str,
        need_whitespace: bool,
        is_whitespace: bool,
        is_indention: bool,
    ) {
        if need_whitespace && !self.whitespace {
            self.put(' ');
        }
        self.write_str(ind);
        self.whitespace = is_whitespace;
        self.indention = self.indention && is_indention;
    }

    fn write_tag_handle(&mut self, handle: &str) {
        if !self.whitespace {
            self.put(' ');
        }
        self.write_str(handle);
        self.whitespace = false;
        self.indention = false;
    }

    /// `yaml_emitter_write_tag_content` (URI characters kept, others `%XX`-escaped per UTF-8 byte).
    fn write_tag_content(&mut self, value: &str) {
        for c in value.chars() {
            if c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '-' | '_'
                        | ';'
                        | '/'
                        | '?'
                        | ':'
                        | '@'
                        | '&'
                        | '='
                        | '+'
                        | '$'
                        | ','
                        | '.'
                        | '~'
                        | '*'
                        | '\''
                        | '('
                        | ')'
                        | '['
                        | ']'
                )
            {
                self.put(c);
            } else {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    write!(self.out, "%{b:02X}").unwrap_or_default();
                    self.column = self.column.saturating_add(3);
                }
            }
        }
        self.whitespace = false;
        self.indention = false;
    }

    fn write_plain_scalar(&mut self, value: &str, allow_breaks: bool) {
        let mut spaces = false;
        let mut breaks = false;
        if !self.whitespace && (!value.is_empty() || self.flow_level != 0) {
            self.put(' ');
        }
        let mut chars = value.chars().peekable();
        while let Some(c) = chars.next() {
            let next = chars.peek().copied();
            if c == ' ' {
                if allow_breaks && !spaces && self.column > BEST_WIDTH && next != Some(' ') {
                    self.write_indent();
                } else {
                    self.put(c);
                }
                spaces = true;
            } else if is_break(c) {
                if !breaks && c == '\n' {
                    self.put_break();
                }
                self.write_break(c);
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.put(c);
                self.indention = false;
                spaces = false;
                breaks = false;
            }
        }
        self.whitespace = false;
        self.indention = false;
    }

    fn write_single_quoted_scalar(&mut self, value: &str, allow_breaks: bool) {
        let mut spaces = false;
        let mut breaks = false;
        self.write_indicator("'", true, false, false);
        let mut chars = value.chars().peekable();
        let mut first = true;
        while let Some(c) = chars.next() {
            let next = chars.peek().copied();
            if c == ' ' {
                if allow_breaks
                    && !spaces
                    && self.column > BEST_WIDTH
                    && !first
                    && next.is_some()
                    && next != Some(' ')
                {
                    self.write_indent();
                } else {
                    self.put(c);
                }
                spaces = true;
            } else if is_break(c) {
                if !breaks && c == '\n' {
                    self.put_break();
                }
                self.write_break(c);
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                if c == '\'' {
                    self.put('\'');
                }
                self.put(c);
                self.indention = false;
                spaces = false;
                breaks = false;
            }
            first = false;
        }
        if breaks {
            self.write_indent();
        }
        self.write_indicator("'", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_double_quoted_scalar(&mut self, value: &str, allow_breaks: bool) {
        let mut spaces = false;
        self.write_indicator("\"", true, false, false);
        let mut chars = value.chars().peekable();
        let mut first = true;
        while let Some(c) = chars.next() {
            let next = chars.peek().copied();
            if !is_printable(c)
                || !c.is_ascii()
                || c == '\u{feff}'
                || is_break(c)
                || c == '"'
                || c == '\\'
            {
                self.put('\\');
                let short = match c {
                    '\0' => Some('0'),
                    '\x07' => Some('a'),
                    '\x08' => Some('b'),
                    '\t' => Some('t'),
                    '\n' => Some('n'),
                    '\x0b' => Some('v'),
                    '\x0c' => Some('f'),
                    '\r' => Some('r'),
                    '\x1b' => Some('e'),
                    '"' => Some('"'),
                    '\\' => Some('\\'),
                    '\u{85}' => Some('N'),
                    '\u{a0}' => Some('_'),
                    '\u{2028}' => Some('L'),
                    '\u{2029}' => Some('P'),
                    _ => None,
                };
                if let Some(s) = short {
                    self.put(s);
                } else {
                    let cp = u32::from(c);
                    let (text, width) = if cp <= 0xff {
                        (format!("x{cp:02X}"), 3)
                    } else if cp <= 0xffff {
                        (format!("u{cp:04X}"), 5)
                    } else {
                        (format!("U{cp:08X}"), 9)
                    };
                    self.out.push_str(&text);
                    self.column = self.column.saturating_add(width);
                }
                spaces = false;
            } else if c == ' ' {
                if allow_breaks && !spaces && self.column > BEST_WIDTH && !first && next.is_some() {
                    self.write_indent();
                    if next == Some(' ') {
                        self.put('\\');
                    }
                } else {
                    self.put(c);
                }
                spaces = true;
            } else {
                self.put(c);
                spaces = false;
            }
            first = false;
        }
        self.write_indicator("\"", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    /// `yaml_emitter_write_block_scalar_hints`.
    fn write_block_scalar_hints(&mut self, value: &str) {
        let first = value.chars().next();
        if first.is_some_and(|c| c == ' ' || is_break(c)) {
            let hint = char::from_digit(u32::try_from(BEST_INDENT).unwrap_or(2), 10).unwrap_or('2');
            let mut buf = [0u8; 4];
            self.write_indicator(hint.encode_utf8(&mut buf), false, false, false);
        }
        self.open_ended = 0;
        let mut rev = value.chars().rev();
        let chomp = match rev.next() {
            None => Some("-"),
            Some(last) if !is_break(last) => Some("-"),
            Some(_) => match rev.next() {
                Some(prev) if !is_break(prev) => None,
                _ => {
                    self.open_ended = 2;
                    Some("+")
                }
            },
        };
        if let Some(chomp) = chomp {
            self.write_indicator(chomp, false, false, false);
        }
    }

    fn write_literal_scalar(&mut self, value: &str) {
        let mut breaks = true;
        self.write_indicator("|", true, false, false);
        self.write_block_scalar_hints(value);
        self.put_break();
        self.indention = true;
        self.whitespace = true;
        for c in value.chars() {
            if is_break(c) {
                self.write_break(c);
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.put(c);
                self.indention = false;
                breaks = false;
            }
        }
    }
}

/// `yaml_emitter_check_simple_key` for the key event (a non-empty collection is never simple).
fn check_simple_key(ev: &Event) -> bool {
    let tag_len = |tag: &str| {
        let t = analyze_tag(tag);
        t.handle.len().saturating_add(t.suffix.len())
    };
    let anchor_len = |anchor: &Option<String>| anchor.as_ref().map_or(0, String::len);
    let length = match ev {
        Event::Alias(name) => name.len(),
        Event::Scalar {
            anchor,
            tag,
            value,
            plain_implicit,
            quoted_implicit,
            ..
        } => {
            if analyze_scalar(value).multiline {
                return false;
            }
            let tag = if !*plain_implicit && !*quoted_implicit {
                tag_len(tag)
            } else {
                0
            };
            anchor_len(anchor)
                .saturating_add(tag)
                .saturating_add(value.len())
        }
        Event::EmptySequence {
            anchor,
            tag,
            implicit,
        }
        | Event::EmptyMapping {
            anchor,
            tag,
            implicit,
        } => anchor_len(anchor).saturating_add(if *implicit { 0 } else { tag_len(tag) }),
        _ => return false,
    };
    length <= 128
}
