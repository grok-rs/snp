//! `Lib/tomllib/_parser.py` + `_re.py`, function for function. Every Python call the original makes is mirrored by
//! [`P::call`]/[`P::ret`] so the emulated frame depth equals CPython's; the value recursion (`parse_value` ↔
//! `parse_array` / `parse_inline_table` / `parse_key_value_pair`) runs on an explicit continuation stack.
//!
//! Owner: WP-17.

use std::collections::HashMap;

use snp_pycompat::repr::{repr_str, repr_str_tuple};

use crate::CheckError;
use crate::json::PyMinor;

/// `sys.getrecursionlimit()`.
const LIMIT: usize = 1000;
/// Frames below `tomllib.loads`'s body in `check-toml`: `<module>`, `main`, `tomllib.load`, `loads`.
const LOADS_DEPTH: usize = 4;
/// 3.13's `MAX_KEY_PARTS` (= the recursion limit).
const MAX_KEY_PARTS: usize = 1000;

const FROZEN: u8 = 1;
const EXPLICIT_NEST: u8 = 2;

type Key = Vec<String>;

/// What a parsed value is, as far as the parser's `isinstance` checks and nests care.
#[derive(Copy, Clone, Debug)]
enum Value {
    /// A `dict` (table) in the arena.
    Table(usize),
    /// A `list` in the arena.
    Array(usize),
    /// Anything else.
    Scalar,
}

/// The `dict`s and `list`s of the document (all `NestedDict`s share it).
#[derive(Debug, Default)]
struct Store {
    tables: Vec<HashMap<String, Value>>,
    arrays: Vec<Vec<Value>>,
}

impl Store {
    fn new_table(&mut self) -> usize {
        self.tables.push(HashMap::new());
        self.tables.len().saturating_sub(1)
    }

    fn new_array(&mut self, items: Vec<Value>) -> usize {
        self.arrays.push(items);
        self.arrays.len().saturating_sub(1)
    }

    /// `NestedDict.get_or_create_nest(key, access_lists=…)` on the dict `root`; `Err` = `KeyError`.
    fn get_or_create_nest(
        &mut self,
        root: usize,
        key: &[String],
        access_lists: bool,
    ) -> Result<usize, ()> {
        let mut cont = Value::Table(root);
        for k in key {
            let Value::Table(t) = cont else {
                return Err(());
            };
            let table = self.tables.get_mut(t).ok_or(())?;
            let next = table.get(k).copied();
            cont = if let Some(v) = next {
                v
            } else {
                let id = self.new_table();
                if let Some(table) = self.tables.get_mut(t) {
                    table.insert(k.clone(), Value::Table(id));
                }
                Value::Table(id)
            };
            if let (true, Value::Array(a)) = (access_lists, cont) {
                // `cont[-1]`; an empty list would be an IndexError (unreachable: the flags forbid it).
                cont = self
                    .arrays
                    .get(a)
                    .and_then(|items| items.last())
                    .copied()
                    .ok_or(())?;
            }
            if !matches!(cont, Value::Table(_)) {
                return Err(());
            }
        }
        match cont {
            Value::Table(t) => Ok(t),
            _ => Err(()),
        }
    }

    /// `NestedDict.append_nest_to_list(key)`.
    fn append_nest_to_list(&mut self, root: usize, key: &[String]) -> Result<(), ()> {
        let (last, parent) = key.split_last().ok_or(())?;
        let cont = self.get_or_create_nest(root, parent, true)?;
        let existing = self.tables.get(cont).and_then(|t| t.get(last)).copied();
        match existing {
            Some(Value::Array(a)) => {
                let id = self.new_table();
                if let Some(items) = self.arrays.get_mut(a) {
                    items.push(Value::Table(id));
                }
                Ok(())
            }
            Some(_) => Err(()),
            None => {
                let id = self.new_table();
                let arr = self.new_array(vec![Value::Table(id)]);
                if let Some(t) = self.tables.get_mut(cont) {
                    t.insert(last.clone(), Value::Array(arr));
                }
                Ok(())
            }
        }
    }

    fn contains(&self, table: usize, key: &str) -> bool {
        self.tables.get(table).is_some_and(|t| t.contains_key(key))
    }

    fn insert(&mut self, table: usize, key: String, v: Value) {
        if let Some(t) = self.tables.get_mut(table) {
            t.insert(key, v);
        }
    }
}

/// One node of `Flags._flags`.
#[derive(Debug, Default)]
struct FlagNode {
    flags: u8,
    recursive: u8,
    nested: HashMap<String, usize>,
}

/// `Flags` (arena of nodes; node 0 is the root whose `nested` is `_flags`).
#[derive(Debug)]
struct Flags {
    nodes: Vec<FlagNode>,
    /// `_pending_flags` as (path, first prefix length, last prefix length) of `EXPLICIT_NEST` entries.
    pending: Vec<(Key, usize, usize)>,
}

impl Flags {
    fn new() -> Self {
        Self {
            nodes: vec![FlagNode::default()],
            pending: Vec::new(),
        }
    }

    fn child(&self, node: usize, k: &str) -> Option<usize> {
        self.nodes.get(node).and_then(|n| n.nested.get(k)).copied()
    }

    fn child_or_create(&mut self, node: usize, k: &str) -> usize {
        if let Some(c) = self.child(node, k) {
            return c;
        }
        let id = self.nodes.len();
        self.nodes.push(FlagNode::default());
        if let Some(n) = self.nodes.get_mut(node) {
            n.nested.insert(k.to_owned(), id);
        }
        id
    }

    /// `set(key, flag, recursive=…)`.
    fn set(&mut self, key: &[String], flag: u8, recursive: bool) {
        let mut cont = 0usize;
        for k in key {
            cont = self.child_or_create(cont, k);
        }
        if let Some(n) = self.nodes.get_mut(cont).filter(|_| !key.is_empty()) {
            if recursive {
                n.recursive |= flag;
            } else {
                n.flags |= flag;
            }
        }
    }

    /// `is_(key[:m], flag)` for every `m` in `1..=key.len()`, in one walk.
    fn is_prefixes(&self, key: &[String], flag: u8) -> Vec<bool> {
        let mut out = Vec::new();
        let mut cont = Some(0usize);
        let mut rec_seen = false;
        for k in key {
            let result = match cont {
                None => false,
                Some(_) if rec_seen => true,
                Some(c) => self
                    .child(c, k)
                    .and_then(|n| self.nodes.get(n))
                    .is_some_and(|n| (n.flags | n.recursive) & flag != 0),
            };
            out.push(result);
            if let (Some(c), false) = (cont, rec_seen) {
                cont = self.child(c, k);
                if let Some(n) = cont.and_then(|n| self.nodes.get(n)) {
                    rec_seen = n.recursive & flag != 0;
                }
            }
        }
        out
    }

    /// `is_(key, flag)`.
    fn is_(&self, key: &[String], flag: u8) -> bool {
        self.is_prefixes(key, flag).last().copied().unwrap_or(false)
    }

    /// `unset_all(key)`.
    fn unset_all(&mut self, key: &[String]) {
        let Some((stem, parent)) = key.split_last() else {
            return;
        };
        let mut cont = 0usize;
        for k in parent {
            match self.child(cont, k) {
                Some(c) => cont = c,
                None => return,
            }
        }
        if let Some(n) = self.nodes.get_mut(cont) {
            n.nested.remove(stem);
        }
    }

    /// `finalize_pending()`.
    fn finalize_pending(&mut self) {
        for (path, from, to) in std::mem::take(&mut self.pending) {
            let mut cont = 0usize;
            for (m, k) in path.iter().enumerate() {
                cont = self.child_or_create(cont, k);
                let len = m.saturating_add(1);
                if let Some(n) = self
                    .nodes
                    .get_mut(cont)
                    .filter(|_| (from..=to).contains(&len))
                {
                    n.flags |= EXPLICIT_NEST;
                }
                if len >= to {
                    break;
                }
            }
        }
    }
}

/// Character classes of the parser.
#[derive(Copy, Clone, Debug)]
enum Chars {
    /// `TOML_WS`.
    Ws,
    /// `TOML_WS_AND_NEWLINE`.
    WsNl,
    /// `BARE_KEY_CHARS`.
    BareKey,
}

impl Chars {
    const fn contains(self, c: char) -> bool {
        match self {
            Self::Ws => matches!(c, ' ' | '\t'),
            Self::WsNl => matches!(c, ' ' | '\t' | '\n'),
            Self::BareKey => c.is_ascii_alphanumeric() || c == '-' || c == '_',
        }
    }
}

/// `ASCII_CTRL` minus the allowed characters.
#[derive(Copy, Clone, Debug)]
enum Illegal {
    /// `ILLEGAL_BASIC_STR_CHARS` (= literal and comment): controls but `\t`.
    Line,
    /// `ILLEGAL_MULTILINE_*`: controls but `\t` and `\n`.
    Multiline,
}

impl Illegal {
    const fn contains(self, c: char) -> bool {
        let ctrl = matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}');
        match self {
            Self::Line => ctrl && c != '\t',
            Self::Multiline => ctrl && c != '\t' && c != '\n',
        }
    }
}

/// A suspended caller of `parse_value`.
enum Cont {
    /// Inside `parse_array`, items so far.
    Array(Vec<Value>),
    /// Inside `parse_inline_table`: its `NestedDict` root and `Flags`.
    Inline { root: usize, flags: Flags },
    /// Inside the inline table's `parse_key_value_pair` call, with its key.
    Pair(Key),
}

struct P {
    src: Vec<char>,
    minor: PyMinor,
    depth: usize,
    store: Store,
}

impl P {
    fn at(&self, pos: usize) -> Option<char> {
        self.src.get(pos).copied()
    }

    fn starts_with(&self, pos: usize, s: &str) -> bool {
        let mut i = pos;
        for c in s.chars() {
            if self.at(i) != Some(c) {
                return false;
            }
            i = i.saturating_add(1);
        }
        true
    }

    fn slice(&self, from: usize, to: usize) -> String {
        self.src
            .get(from..to.min(self.src.len()))
            .map_or_default(|s| s.iter().collect())
    }

    /// Entering a Python function.
    fn call(&mut self) -> Result<(), CheckError> {
        if self.depth >= LIMIT {
            return Err(CheckError::crash(
                "RecursionError",
                "maximum recursion depth exceeded",
            ));
        }
        self.depth = self.depth.saturating_add(1);
        Ok(())
    }

    /// Leaving it.
    fn ret(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// `raise suffixed_err(src, pos, msg)` (two more frames: `suffixed_err`, `coord_repr`).
    fn err(&self, pos: usize, msg: &str) -> CheckError {
        if self.depth.saturating_add(2) > LIMIT {
            return CheckError::crash("RecursionError", "maximum recursion depth exceeded");
        }
        let coords = if pos >= self.src.len() {
            "end of document".to_owned()
        } else {
            let before = self.src.get(..pos).unwrap_or_default();
            let line = before
                .iter()
                .filter(|&&c| c == '\n')
                .count()
                .saturating_add(1);
            let column = if line == 1 {
                pos.saturating_add(1)
            } else {
                pos.saturating_sub(before.iter().rposition(|&c| c == '\n').unwrap_or(0))
            };
            format!("line {line}, column {column}")
        };
        CheckError::Reported(format!("{msg} (at {coords})"))
    }

    fn skip_chars(&mut self, mut pos: usize, chars: Chars) -> Result<usize, CheckError> {
        self.call()?;
        while self.at(pos).is_some_and(|c| chars.contains(c)) {
            pos = pos.saturating_add(1);
        }
        self.ret();
        Ok(pos)
    }

    /// `skip_until(src, pos, expect, error_on=…, error_on_eof=…)`.
    fn skip_until(
        &mut self,
        pos: usize,
        expect: &str,
        error_on: Illegal,
        eof_err: bool,
    ) -> Result<usize, CheckError> {
        self.call()?;
        let mut new_pos = None;
        let mut i = pos;
        while i < self.src.len() {
            if self.starts_with(i, expect) {
                new_pos = Some(i);
                break;
            }
            i = i.saturating_add(1);
        }
        let new_pos = if let Some(p) = new_pos {
            p
        } else {
            let len = self.src.len();
            if eof_err {
                return Err(self.err(len, &format!("Expected {}", repr_str(expect))));
            }
            len
        };
        if let Some(bad) =
            (pos..new_pos).find(|&p| self.at(p).is_some_and(|c| error_on.contains(c)))
        {
            let c = self.at(bad).unwrap_or_default();
            let mut tmp = [0u8; 4];
            return Err(self.err(
                bad,
                &format!(
                    "Found invalid character {}",
                    repr_str(c.encode_utf8(&mut tmp))
                ),
            ));
        }
        self.ret();
        Ok(new_pos)
    }

    fn skip_comment(&mut self, pos: usize) -> Result<usize, CheckError> {
        self.call()?;
        let out = if self.at(pos) == Some('#') {
            self.skip_until(pos.saturating_add(1), "\n", Illegal::Line, false)?
        } else {
            pos
        };
        self.ret();
        Ok(out)
    }

    fn skip_comments_and_array_ws(&mut self, mut pos: usize) -> Result<usize, CheckError> {
        self.call()?;
        loop {
            let before = pos;
            pos = self.skip_chars(pos, Chars::WsNl)?;
            pos = self.skip_comment(pos)?;
            if pos == before {
                break;
            }
        }
        self.ret();
        Ok(pos)
    }

    // ----------------------------------------------------------------------------------------------------------
    // keys and strings
    // ----------------------------------------------------------------------------------------------------------

    fn parse_key(&mut self, pos: usize) -> Result<(usize, Key), CheckError> {
        self.call()?;
        let (mut pos, part) = self.parse_key_part(pos)?;
        let mut key = vec![part];
        pos = self.skip_chars(pos, Chars::Ws)?;
        while self.at(pos) == Some('.') {
            pos = self.skip_chars(pos.saturating_add(1), Chars::Ws)?;
            let (p, part) = self.parse_key_part(pos)?;
            pos = p;
            key.push(part);
            if matches!(self.minor, PyMinor::P313) && key.len() > MAX_KEY_PARTS {
                return Err(CheckError::crash(
                    "RecursionError",
                    format!("TOML key has more than the allowed {MAX_KEY_PARTS} parts"),
                ));
            }
            pos = self.skip_chars(pos, Chars::Ws)?;
        }
        self.ret();
        Ok((pos, key))
    }

    fn parse_key_part(&mut self, pos: usize) -> Result<(usize, String), CheckError> {
        self.call()?;
        let out = match self.at(pos) {
            Some(c) if Chars::BareKey.contains(c) => {
                let end = self.skip_chars(pos, Chars::BareKey)?;
                (end, self.slice(pos, end))
            }
            Some('\'') => self.parse_literal_str(pos)?,
            Some('"') => self.parse_one_line_basic_str(pos)?,
            _ => return Err(self.err(pos, "Invalid initial character for a key part")),
        };
        self.ret();
        Ok(out)
    }

    fn parse_one_line_basic_str(&mut self, pos: usize) -> Result<(usize, String), CheckError> {
        self.call()?;
        let out = self.parse_basic_str(pos.saturating_add(1), false)?;
        self.ret();
        Ok(out)
    }

    fn parse_literal_str(&mut self, pos: usize) -> Result<(usize, String), CheckError> {
        self.call()?;
        let start = pos.saturating_add(1);
        let end = self.skip_until(start, "'", Illegal::Line, true)?;
        self.ret();
        Ok((end.saturating_add(1), self.slice(start, end)))
    }

    fn parse_multiline_str(
        &mut self,
        pos: usize,
        literal: bool,
    ) -> Result<(usize, String), CheckError> {
        self.call()?;
        let mut pos = pos.saturating_add(3);
        if self.at(pos) == Some('\n') {
            pos = pos.saturating_add(1);
        }
        let (delim, mut result) = if literal {
            let end = self.skip_until(pos, "'''", Illegal::Multiline, true)?;
            let r = self.slice(pos, end);
            pos = end.saturating_add(3);
            ('\'', r)
        } else {
            let (p, r) = self.parse_basic_str(pos, true)?;
            pos = p;
            ('"', r)
        };
        for _ in 0..2 {
            if self.at(pos) != Some(delim) {
                break;
            }
            pos = pos.saturating_add(1);
            result.push(delim);
        }
        self.ret();
        Ok((pos, result))
    }

    fn parse_basic_str(
        &mut self,
        mut pos: usize,
        multiline: bool,
    ) -> Result<(usize, String), CheckError> {
        self.call()?;
        let error_on = if multiline {
            Illegal::Multiline
        } else {
            Illegal::Line
        };
        let mut result = String::new();
        let mut start = pos;
        loop {
            let Some(c) = self.at(pos) else {
                return Err(self.err(pos, "Unterminated string"));
            };
            if c == '"' {
                if !multiline {
                    result.push_str(&self.slice(start, pos));
                    self.ret();
                    return Ok((pos.saturating_add(1), result));
                }
                if self.starts_with(pos, "\"\"\"") {
                    result.push_str(&self.slice(start, pos));
                    self.ret();
                    return Ok((pos.saturating_add(3), result));
                }
                pos = pos.saturating_add(1);
                continue;
            }
            if c == '\\' {
                result.push_str(&self.slice(start, pos));
                let (p, s) = if multiline {
                    self.call()?; // parse_basic_str_escape_multiline
                    let r = self.parse_basic_str_escape(pos, true)?;
                    self.ret();
                    r
                } else {
                    self.parse_basic_str_escape(pos, false)?
                };
                pos = p;
                result.push_str(&s);
                start = pos;
                continue;
            }
            if error_on.contains(c) {
                let mut tmp = [0u8; 4];
                return Err(self.err(
                    pos,
                    &format!("Illegal character {}", repr_str(c.encode_utf8(&mut tmp))),
                ));
            }
            pos = pos.saturating_add(1);
        }
    }

    fn parse_basic_str_escape(
        &mut self,
        pos: usize,
        multiline: bool,
    ) -> Result<(usize, String), CheckError> {
        self.call()?;
        let escape_id = self.slice(pos, pos.saturating_add(2));
        let mut pos = pos.saturating_add(2);
        if multiline && matches!(escape_id.as_str(), "\\ " | "\\\t" | "\\\n") {
            if escape_id != "\\\n" {
                pos = self.skip_chars(pos, Chars::Ws)?;
                match self.at(pos) {
                    None => {
                        self.ret();
                        return Ok((pos, String::new()));
                    }
                    Some('\n') => pos = pos.saturating_add(1),
                    Some(_) => return Err(self.err(pos, "Unescaped '\\' in a string")),
                }
            }
            pos = self.skip_chars(pos, Chars::WsNl)?;
            self.ret();
            return Ok((pos, String::new()));
        }
        let out = match escape_id.as_str() {
            "\\u" => self.parse_hex_char(pos, 4)?,
            "\\U" => self.parse_hex_char(pos, 8)?,
            "\\b" => (pos, "\u{8}".to_owned()),
            "\\t" => (pos, "\t".to_owned()),
            "\\n" => (pos, "\n".to_owned()),
            "\\f" => (pos, "\u{c}".to_owned()),
            "\\r" => (pos, "\r".to_owned()),
            "\\\"" => (pos, "\"".to_owned()),
            "\\\\" => (pos, "\\".to_owned()),
            _ => return Err(self.err(pos, "Unescaped '\\' in a string")),
        };
        self.ret();
        Ok(out)
    }

    fn parse_hex_char(&mut self, pos: usize, len: usize) -> Result<(usize, String), CheckError> {
        self.call()?;
        let hex = self.slice(pos, pos.saturating_add(len));
        if hex.chars().count() != len || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(self.err(pos, "Invalid hex value"));
        }
        let pos = pos.saturating_add(len);
        let value = u32::from_str_radix(&hex, 16).unwrap_or(u32::MAX);
        // is_unicode_scalar_value(...)
        self.call()?;
        self.ret();
        let Some(c) = char::from_u32(value) else {
            return Err(self.err(pos, "Escaped character is not a Unicode scalar value"));
        };
        self.ret();
        Ok((pos, c.to_string()))
    }

    // ----------------------------------------------------------------------------------------------------------
    // scalar values
    // ----------------------------------------------------------------------------------------------------------

    fn digits(&self, pos: usize, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for i in 0..n {
            v = v
                .checked_mul(10)?
                .checked_add(self.at(pos.checked_add(i)?)?.to_digit(10)?)?;
        }
        Some(v)
    }

    /// `_TIME_RE_STR` at `pos`: the end of the match.
    fn match_time(&self, pos: usize) -> Option<usize> {
        let hh = self.digits(pos, 2)?;
        let mm = self.digits(pos.saturating_add(3), 2)?;
        let ss = self.digits(pos.saturating_add(6), 2)?;
        if hh > 23
            || mm > 59
            || ss > 59
            || self.at(pos.saturating_add(2)) != Some(':')
            || self.at(pos.saturating_add(5)) != Some(':')
        {
            return None;
        }
        let mut end = pos.saturating_add(8);
        if self.at(end) == Some('.')
            && self
                .at(end.saturating_add(1))
                .is_some_and(|c| c.is_ascii_digit())
        {
            end = end.saturating_add(1);
            while self.at(end).is_some_and(|c| c.is_ascii_digit()) {
                end = end.saturating_add(1);
            }
        }
        Some(end)
    }

    /// `RE_DATETIME.match(src, pos)`: (end, year, month, day, has time, has offset).
    fn match_datetime(&self, pos: usize) -> Option<(usize, u32, u32, u32, bool, bool)> {
        let year = self.digits(pos, 4)?;
        let month = self.digits(pos.saturating_add(5), 2)?;
        let day = self.digits(pos.saturating_add(8), 2)?;
        if self.at(pos.saturating_add(4)) != Some('-')
            || self.at(pos.saturating_add(7)) != Some('-')
            || !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
        {
            return None;
        }
        let date_end = pos.saturating_add(10);
        if !matches!(self.at(date_end), Some('T' | 't' | ' ')) {
            return Some((date_end, year, month, day, false, false));
        }
        let Some(mut end) = self.match_time(date_end.saturating_add(1)) else {
            return Some((date_end, year, month, day, false, false));
        };
        let mut offset = false;
        match self.at(end) {
            Some('Z' | 'z') => end = end.saturating_add(1),
            Some('+' | '-') => {
                let hh = self.digits(end.saturating_add(1), 2);
                let mm = self.digits(end.saturating_add(4), 2);
                let colon = self.at(end.saturating_add(3)) == Some(':');
                if let (Some(0..=23), Some(0..=59), true) = (hh, mm, colon) {
                    end = end.saturating_add(6);
                    offset = true;
                }
            }
            _ => {}
        }
        Some((end, year, month, day, true, offset))
    }

    /// `RE_NUMBER.match(src, pos)`: (end, is float).
    fn match_number(&self, pos: usize) -> Option<(usize, bool)> {
        let run = |mut i: usize, ok: &dyn Fn(char) -> bool| -> usize {
            // (?:_?[digit])*
            loop {
                match self.at(i) {
                    Some(c) if ok(c) => i = i.saturating_add(1),
                    Some('_') if self.at(i.saturating_add(1)).is_some_and(ok) => {
                        i = i.saturating_add(2);
                    }
                    _ => return i,
                }
            }
        };
        let dec = |c: char| c.is_ascii_digit();
        if self.at(pos) == Some('0') {
            let (kind, next) = (
                self.at(pos.saturating_add(1)),
                self.at(pos.saturating_add(2)),
            );
            let digit_ok: Option<&dyn Fn(char) -> bool> = match kind {
                Some('x') => Some(&|c: char| c.is_ascii_hexdigit()),
                Some('b') => Some(&|c: char| c == '0' || c == '1'),
                Some('o') => Some(&|c: char| ('0'..='7').contains(&c)),
                _ => None,
            };
            if let Some(ok) = digit_ok.filter(|ok| next.is_some_and(ok)) {
                return Some((run(pos.saturating_add(3), ok), false));
            }
        }
        let mut i = pos;
        if matches!(self.at(i), Some('+' | '-')) {
            i = i.saturating_add(1);
        }
        match self.at(i) {
            Some('0') => i = i.saturating_add(1),
            Some('1'..='9') => i = run(i.saturating_add(1), &dec),
            _ => return None,
        }
        let int_end = i;
        if self.at(i) == Some('.') && self.at(i.saturating_add(1)).is_some_and(dec) {
            i = run(i.saturating_add(2), &dec);
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut j = i.saturating_add(1);
            if matches!(self.at(j), Some('+' | '-')) {
                j = j.saturating_add(1);
            }
            if self.at(j).is_some_and(dec) {
                i = run(j.saturating_add(1), &dec);
            }
        }
        Some((i, i != int_end))
    }

    /// The scalar branches of `parse_value` (the char is not `[` or `{`): `Ok((end, Value::Scalar))`.
    fn parse_scalar(&mut self, pos: usize) -> Result<(usize, Value), CheckError> {
        let c = self.at(pos);
        if c == Some('"') {
            let (end, _) = if self.starts_with(pos, "\"\"\"") {
                self.parse_multiline_str(pos, false)?
            } else {
                self.parse_one_line_basic_str(pos)?
            };
            return Ok((end, Value::Scalar));
        }
        if c == Some('\'') {
            let (end, _) = if self.starts_with(pos, "'''") {
                self.parse_multiline_str(pos, true)?
            } else {
                self.parse_literal_str(pos)?
            };
            return Ok((end, Value::Scalar));
        }
        if c == Some('t') && self.starts_with(pos, "true") {
            return Ok((pos.saturating_add(4), Value::Scalar));
        }
        if c == Some('f') && self.starts_with(pos, "false") {
            return Ok((pos.saturating_add(5), Value::Scalar));
        }
        if let Some((end, year, month, day, has_time, has_offset)) = self.match_datetime(pos) {
            // match_to_datetime(...): date()/datetime() raise ValueError for impossible dates.
            self.call()?;
            if !valid_date(year, month, day) {
                self.ret();
                return Err(self.err(pos, "Invalid date or datetime"));
            }
            if has_time && has_offset {
                self.call()?; // cached_tz
                self.ret();
            }
            self.ret();
            return Ok((end, Value::Scalar));
        }
        if let Some(end) = self.match_time(pos) {
            self.call()?; // match_to_localtime
            self.ret();
            return Ok((end, Value::Scalar));
        }
        if let Some((end, is_float)) = self.match_number(pos) {
            self.call()?; // match_to_number
            if !is_float {
                let text = self.slice(pos, end);
                let body = text.trim_start_matches(['+', '-']);
                let decimal =
                    !(body.starts_with("0x") || body.starts_with("0o") || body.starts_with("0b"));
                let digits = body.chars().filter(char::is_ascii_digit).count();
                if decimal && digits > 4300 {
                    return Err(CheckError::crash(
                        "ValueError",
                        format!(
                            "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} \
                             digits; use sys.set_int_max_str_digits() to increase the limit"
                        ),
                    ));
                }
            }
            self.ret();
            return Ok((end, Value::Scalar));
        }
        let three = self.slice(pos, pos.saturating_add(3));
        if three == "inf" || three == "nan" {
            return Ok((pos.saturating_add(3), Value::Scalar));
        }
        let four = self.slice(pos, pos.saturating_add(4));
        if matches!(four.as_str(), "-inf" | "+inf" | "-nan" | "+nan") {
            return Ok((pos.saturating_add(4), Value::Scalar));
        }
        Err(self.err(pos, "Invalid value"))
    }

    // ----------------------------------------------------------------------------------------------------------
    // parse_value and the containers
    // ----------------------------------------------------------------------------------------------------------

    /// `parse_key_value_pair(src, pos)` up to the `parse_value` call: the key and the value position. The caller
    /// has entered the function's frame.
    fn kvp_head(&mut self, pos: usize) -> Result<(usize, Key), CheckError> {
        let (pos, key) = self.parse_key(pos)?;
        if self.at(pos) != Some('=') {
            return Err(self.err(pos, "Expected '=' after a key in a key/value pair"));
        }
        let pos = self.skip_chars(pos.saturating_add(1), Chars::Ws)?;
        Ok((pos, key))
    }

    /// `parse_value(src, pos)` (the caller's frame is current); iterative over the nesting.
    fn parse_value(&mut self, pos: usize) -> Result<(usize, Value), CheckError> {
        let mut conts: Vec<Cont> = Vec::new();
        let mut pos = pos;
        'value: loop {
            self.call()?; // parse_value
            let (mut end, mut val) = match self.at(pos) {
                Some('[') => {
                    self.call()?; // parse_array
                    let p = self.skip_comments_and_array_ws(pos.saturating_add(1))?;
                    if self.at(p) == Some(']') {
                        let id = self.store.new_array(Vec::new());
                        self.ret();
                        self.ret();
                        (p.saturating_add(1), Value::Array(id))
                    } else {
                        conts.push(Cont::Array(Vec::new()));
                        pos = p;
                        continue 'value;
                    }
                }
                Some('{') => {
                    self.call()?; // parse_inline_table
                    self.call()?; // NestedDict()
                    self.ret();
                    self.call()?; // Flags()
                    self.ret();
                    let root = self.store.new_table();
                    let p = self.skip_chars(pos.saturating_add(1), Chars::Ws)?;
                    if self.at(p) == Some('}') {
                        self.ret();
                        self.ret();
                        (p.saturating_add(1), Value::Table(root))
                    } else {
                        conts.push(Cont::Inline {
                            root,
                            flags: Flags::new(),
                        });
                        self.call()?; // parse_key_value_pair
                        let (p, key) = self.kvp_head(p)?;
                        conts.push(Cont::Pair(key));
                        pos = p;
                        continue 'value;
                    }
                }
                _ => {
                    let r = self.parse_scalar(pos)?;
                    self.ret();
                    r
                }
            };
            // Deliver (end, val) to the suspended callers.
            loop {
                match conts.pop() {
                    None => return Ok((end, val)),
                    Some(Cont::Array(mut items)) => {
                        items.push(val);
                        let p = self.skip_comments_and_array_ws(end)?;
                        let close = match self.at(p) {
                            Some(']') => Some(p),
                            Some(',') => {
                                let q = self.skip_comments_and_array_ws(p.saturating_add(1))?;
                                (self.at(q) == Some(']')).then_some(q).or_else(|| {
                                    pos = q;
                                    None
                                })
                            }
                            _ => return Err(self.err(p, "Unclosed array")),
                        };
                        if let Some(q) = close {
                            let id = self.store.new_array(items);
                            self.ret(); // parse_array
                            self.ret(); // parse_value
                            end = q.saturating_add(1);
                            val = Value::Array(id);
                            continue;
                        }
                        conts.push(Cont::Array(items));
                        continue 'value;
                    }
                    Some(Cont::Pair(key)) => {
                        self.ret(); // parse_key_value_pair
                        let Some(Cont::Inline { root, mut flags }) = conts.pop() else {
                            return Err(CheckError::crash(
                                "SystemError",
                                "toml continuation mismatch",
                            ));
                        };
                        let (stem, parent) = key
                            .split_last()
                            .map_or(("", &[][..]), |(s, p)| (s.as_str(), p));
                        self.call()?; // flags.is_
                        let frozen = flags.is_(&key, FROZEN);
                        self.ret();
                        if frozen {
                            return Err(self.err(
                                end,
                                &format!(
                                    "Cannot mutate immutable namespace {}",
                                    repr_str_tuple(&key)
                                ),
                            ));
                        }
                        self.call()?; // get_or_create_nest
                        let nest = self.store.get_or_create_nest(root, parent, false);
                        self.ret();
                        let Ok(nest) = nest else {
                            return Err(self.err(end, "Cannot overwrite a value"));
                        };
                        if self.store.contains(nest, stem) {
                            return Err(self.err(
                                end,
                                &format!("Duplicate inline table key {}", repr_str(stem)),
                            ));
                        }
                        self.store.insert(nest, stem.to_owned(), val);
                        let p = self.skip_chars(end, Chars::Ws)?;
                        match self.at(p) {
                            Some('}') => {
                                self.ret(); // parse_inline_table
                                self.ret(); // parse_value
                                end = p.saturating_add(1);
                                val = Value::Table(root);
                                continue;
                            }
                            Some(',') => {}
                            _ => return Err(self.err(p, "Unclosed inline table")),
                        }
                        if matches!(val, Value::Table(_) | Value::Array(_)) {
                            self.call()?; // flags.set
                            flags.set(&key, FROZEN, true);
                            self.ret();
                        }
                        let p = self.skip_chars(p.saturating_add(1), Chars::Ws)?;
                        conts.push(Cont::Inline { root, flags });
                        self.call()?; // parse_key_value_pair
                        let (p, key) = self.kvp_head(p)?;
                        conts.push(Cont::Pair(key));
                        pos = p;
                        continue 'value;
                    }
                    Some(Cont::Inline { .. }) => {
                        return Err(CheckError::crash(
                            "SystemError",
                            "toml continuation mismatch",
                        ));
                    }
                }
            }
        }
    }
}

/// `datetime.date(year, month, day)` accepts it (`MINYEAR` is 1).
fn valid_date(year: u32, month: u32, day: u32) -> bool {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    year >= 1 && day >= 1 && day <= days
}

/// `tomllib.loads(s)` as `check-toml` calls it.
pub(super) fn loads(text: &str, minor: PyMinor) -> Result<(), CheckError> {
    let src: Vec<char> = text.replace("\r\n", "\n").chars().collect();
    let mut p = P {
        src,
        minor,
        depth: LOADS_DEPTH,
        store: Store::default(),
    };
    let root = p.store.new_table();
    let mut flags = Flags::new();
    let mut header: Key = Vec::new();
    let mut pos = 0usize;
    loop {
        pos = p.skip_chars(pos, Chars::Ws)?;
        let Some(c) = p.at(pos) else { break };
        if c == '\n' {
            pos = pos.saturating_add(1);
            continue;
        }
        if Chars::BareKey.contains(c) || c == '"' || c == '\'' {
            pos = key_value_rule(&mut p, &mut flags, root, &header, pos)?;
            pos = p.skip_chars(pos, Chars::Ws)?;
        } else if c == '[' {
            let list = p.at(pos.saturating_add(1)) == Some('[');
            flags.finalize_pending();
            let (end, key) = if list {
                create_list_rule(&mut p, &mut flags, root, pos)?
            } else {
                create_dict_rule(&mut p, &mut flags, root, pos)?
            };
            header = key;
            pos = p.skip_chars(end, Chars::Ws)?;
        } else if c != '#' {
            return Err(p.err(pos, "Invalid statement"));
        }
        pos = p.skip_comment(pos)?;
        match p.at(pos) {
            None => break,
            Some('\n') => pos = pos.saturating_add(1),
            Some(_) => {
                return Err(p.err(pos, "Expected newline or end of document after a statement"));
            }
        }
    }
    Ok(())
}

fn key_value_rule(
    p: &mut P,
    flags: &mut Flags,
    root: usize,
    header: &[String],
    pos: usize,
) -> Result<usize, CheckError> {
    p.call()?; // key_value_rule
    p.call()?; // parse_key_value_pair
    let (vpos, key) = p.kvp_head(pos)?;
    let (pos, value) = p.parse_value(vpos)?;
    p.ret();
    let Some((stem, key_parent)) = key.split_last() else {
        return Err(p.err(pos, "Invalid statement"));
    };
    let mut full: Key = header.to_vec();
    full.extend(key.iter().cloned());
    // Relative path containers: header + key[:i] for i in 1..len(key).
    if key.len() > 1 {
        let from = header.len().saturating_add(1);
        let to = header.len().saturating_add(key_parent.len());
        let explicit = flags.is_prefixes(&full, EXPLICIT_NEST);
        for len in from..=to {
            if explicit
                .get(len.saturating_sub(1))
                .copied()
                .unwrap_or(false)
            {
                let cont_key = full.get(..len).unwrap_or_default();
                return Err(p.err(
                    pos,
                    &format!("Cannot redefine namespace {}", repr_str_tuple(cont_key)),
                ));
            }
        }
        flags.pending.push((full.clone(), from, to));
    }
    let abs_parent = full.get(..full.len().saturating_sub(1)).unwrap_or_default();
    if flags.is_(abs_parent, FROZEN) {
        return Err(p.err(
            pos,
            &format!(
                "Cannot mutate immutable namespace {}",
                repr_str_tuple(abs_parent)
            ),
        ));
    }
    let Ok(nest) = p.store.get_or_create_nest(root, abs_parent, true) else {
        return Err(p.err(pos, "Cannot overwrite a value"));
    };
    if p.store.contains(nest, stem) {
        return Err(p.err(pos, "Cannot overwrite a value"));
    }
    if matches!(value, Value::Table(_) | Value::Array(_)) {
        flags.set(&full, FROZEN, true);
    }
    p.store.insert(nest, stem.clone(), value);
    p.ret();
    Ok(pos)
}

fn create_dict_rule(
    p: &mut P,
    flags: &mut Flags,
    root: usize,
    pos: usize,
) -> Result<(usize, Key), CheckError> {
    p.call()?;
    let pos = p.skip_chars(pos.saturating_add(1), Chars::Ws)?;
    let (pos, key) = p.parse_key(pos)?;
    if flags.is_(&key, EXPLICIT_NEST) || flags.is_(&key, FROZEN) {
        return Err(p.err(
            pos,
            &format!("Cannot declare {} twice", repr_str_tuple(&key)),
        ));
    }
    flags.set(&key, EXPLICIT_NEST, false);
    if p.store.get_or_create_nest(root, &key, true).is_err() {
        return Err(p.err(pos, "Cannot overwrite a value"));
    }
    if p.at(pos) != Some(']') {
        return Err(p.err(pos, "Expected ']' at the end of a table declaration"));
    }
    p.ret();
    Ok((pos.saturating_add(1), key))
}

fn create_list_rule(
    p: &mut P,
    flags: &mut Flags,
    root: usize,
    pos: usize,
) -> Result<(usize, Key), CheckError> {
    p.call()?;
    let pos = p.skip_chars(pos.saturating_add(2), Chars::Ws)?;
    let (pos, key) = p.parse_key(pos)?;
    if flags.is_(&key, FROZEN) {
        return Err(p.err(
            pos,
            &format!("Cannot mutate immutable namespace {}", repr_str_tuple(&key)),
        ));
    }
    flags.unset_all(&key);
    flags.set(&key, EXPLICIT_NEST, false);
    if p.store.append_nest_to_list(root, &key).is_err() {
        return Err(p.err(pos, "Cannot overwrite a value"));
    }
    if !p.starts_with(pos, "]]") {
        return Err(p.err(pos, "Expected ']]' at the end of an array declaration"));
    }
    p.ret();
    Ok((pos.saturating_add(2), key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_prefixes() {
        let mut f = Flags::new();
        let k = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        f.set(&k(&["a", "b"]), FROZEN, true);
        assert!(f.is_(&k(&["a", "b"]), FROZEN));
        assert!(f.is_(&k(&["a", "b", "c", "d"]), FROZEN));
        assert!(!f.is_(&k(&["a"]), FROZEN));
        assert_eq!(
            f.is_prefixes(&k(&["a", "b", "c"]), FROZEN),
            [false, true, true]
        );
        f.pending.push((k(&["x", "y", "z"]), 1, 2));
        f.finalize_pending();
        assert_eq!(
            f.is_prefixes(&k(&["x", "y", "z"]), EXPLICIT_NEST),
            [true, true, false]
        );
        f.unset_all(&k(&["x", "y"]));
        assert_eq!(f.is_prefixes(&k(&["x", "y"]), EXPLICIT_NEST), [true, false]);
    }

    #[test]
    fn dates() {
        assert!(valid_date(2024, 2, 29));
        assert!(!valid_date(2023, 2, 29));
        assert!(!valid_date(1900, 2, 29));
        assert!(valid_date(2000, 2, 29));
        assert!(!valid_date(0, 1, 1));
        assert!(!valid_date(2021, 4, 31));
    }
}
