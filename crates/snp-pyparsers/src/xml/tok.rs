//! expat's tokenizer (`xmltok_impl.c`): `prologTok`, `contentTok`, `cdataSectionTok`, `attributeValueTok`,
//! `entityValueTok`, `ignoreSectionTok` and their scanners, over one encoding and byte buffer. Every function
//! returns expat's token number and the `nextTokPtr` it sets (`None` when it leaves it unset).
//!
//! Owner: WP-17.

use super::enc::{Bt, Enc};

pub(super) const TRAILING_RSQB: i32 = -5;
pub(super) const NONE: i32 = -4;
pub(super) const TRAILING_CR: i32 = -3;
pub(super) const PARTIAL_CHAR: i32 = -2;
pub(super) const PARTIAL: i32 = -1;
pub(super) const INVALID: i32 = 0;
pub(super) const START_TAG_WITH_ATTS: i32 = 1;
pub(super) const START_TAG_NO_ATTS: i32 = 2;
pub(super) const EMPTY_ELEMENT_WITH_ATTS: i32 = 3;
pub(super) const EMPTY_ELEMENT_NO_ATTS: i32 = 4;
pub(super) const END_TAG: i32 = 5;
pub(super) const DATA_CHARS: i32 = 6;
pub(super) const DATA_NEWLINE: i32 = 7;
pub(super) const CDATA_SECT_OPEN: i32 = 8;
pub(super) const ENTITY_REF: i32 = 9;
pub(super) const CHAR_REF: i32 = 10;
pub(super) const PI: i32 = 11;
pub(super) const XML_DECL: i32 = 12;
pub(super) const COMMENT: i32 = 13;
pub(super) const BOM: i32 = 14;
pub(super) const PROLOG_S: i32 = 15;
pub(super) const DECL_OPEN: i32 = 16;
pub(super) const DECL_CLOSE: i32 = 17;
pub(super) const NAME: i32 = 18;
pub(super) const NMTOKEN: i32 = 19;
pub(super) const POUND_NAME: i32 = 20;
pub(super) const OR: i32 = 21;
pub(super) const PERCENT: i32 = 22;
pub(super) const OPEN_PAREN: i32 = 23;
pub(super) const CLOSE_PAREN: i32 = 24;
pub(super) const OPEN_BRACKET: i32 = 25;
pub(super) const CLOSE_BRACKET: i32 = 26;
pub(super) const LITERAL: i32 = 27;
pub(super) const PARAM_ENTITY_REF: i32 = 28;
pub(super) const INSTANCE_START: i32 = 29;
pub(super) const NAME_QUESTION: i32 = 30;
pub(super) const NAME_ASTERISK: i32 = 31;
pub(super) const NAME_PLUS: i32 = 32;
pub(super) const COND_SECT_OPEN: i32 = 33;
pub(super) const COND_SECT_CLOSE: i32 = 34;
pub(super) const CLOSE_PAREN_QUESTION: i32 = 35;
pub(super) const CLOSE_PAREN_ASTERISK: i32 = 36;
pub(super) const CLOSE_PAREN_PLUS: i32 = 37;
pub(super) const COMMA: i32 = 38;
pub(super) const ATTRIBUTE_VALUE_S: i32 = 39;
pub(super) const CDATA_SECT_CLOSE: i32 = 40;

/// A token: the number and `nextTokPtr`.
pub(super) type Tok = (i32, Option<usize>);

/// The tokenizer state: an encoding over a buffer.
pub(super) struct Tk<'a> {
    pub(super) enc: &'a Enc,
    pub(super) buf: &'a [u8],
}

const fn tok(t: i32, next: usize) -> Tok {
    (t, Some(next))
}

const fn bare(t: i32) -> Tok {
    (t, None)
}

/// Result of a name-character check (`CHECK_NAME_CASES` / `CHECK_NMSTRT_CASES`).
enum NameStep {
    /// A name character of this many bytes.
    Ok(usize),
    /// Not a name character (the scanner decides).
    No,
    /// An early return of the scanner.
    Ret(Tok),
}

impl Tk<'_> {
    fn m(&self) -> usize {
        self.enc.minbpc()
    }

    fn bt(&self, p: usize) -> Bt {
        self.enc.bt(self.buf, p)
    }

    fn has_chars(&self, p: usize, end: usize, n: usize) -> bool {
        end.saturating_sub(p) >= n.saturating_mul(self.m()) && p <= end
    }

    fn has_char(&self, p: usize, end: usize) -> bool {
        self.has_chars(p, end, 1)
    }

    fn matches(&self, p: usize, c: u8) -> bool {
        self.enc.char_matches(self.buf, p, c)
    }

    /// `CHECK_NAME_CASES` (`start == false`) / `CHECK_NMSTRT_CASES` (`start == true`) at `p`.
    fn name_step(&self, p: usize, end: usize, start: bool) -> NameStep {
        let m = self.m();
        match self.bt(p) {
            Bt::Nonascii => {
                let ok = if start {
                    self.enc.is_nmstrt_char_minbpc(self.buf, p)
                } else {
                    self.enc.is_name_char_minbpc(self.buf, p)
                };
                if ok {
                    NameStep::Ok(m)
                } else {
                    NameStep::Ret(tok(INVALID, p))
                }
            }
            Bt::Nmstrt | Bt::Hex => NameStep::Ok(m),
            Bt::Digit | Bt::Name | Bt::Minus if !start => NameStep::Ok(m),
            t @ (Bt::Lead2 | Bt::Lead3 | Bt::Lead4) => {
                let n = match t {
                    Bt::Lead2 => 2,
                    Bt::Lead3 => 3,
                    _ => 4,
                };
                if end.saturating_sub(p) < n {
                    return NameStep::Ret(bare(PARTIAL_CHAR));
                }
                if self.enc.is_invalid_char(self.buf, p, n) {
                    return NameStep::Ret(tok(INVALID, p));
                }
                let ok = if start {
                    self.enc.is_nmstrt_char(self.buf, p, n)
                } else {
                    self.enc.is_name_char(self.buf, p, n)
                };
                if ok {
                    NameStep::Ok(n)
                } else {
                    NameStep::Ret(tok(INVALID, p))
                }
            }
            _ => NameStep::No,
        }
    }

    /// `INVALID_CASES`: `Some(token)` to return, or the number of bytes to skip.
    fn invalid_case(&self, p: usize, end: usize) -> Result<Option<usize>, Tok> {
        let t = self.bt(p);
        let n = match t {
            Bt::Lead2 => 2,
            Bt::Lead3 => 3,
            Bt::Lead4 => 4,
            Bt::Nonxml | Bt::Malform | Bt::Trail => return Err(tok(INVALID, p)),
            _ => return Ok(None),
        };
        if end.saturating_sub(p) < n {
            return Err(bare(PARTIAL_CHAR));
        }
        if self.enc.is_invalid_char(self.buf, p, n) {
            return Err(tok(INVALID, p));
        }
        Ok(Some(n))
    }

    /// Truncates `end` to whole characters for 2-byte encodings (`None`: no whole character left).
    fn align(&self, p: usize, end: usize) -> Option<usize> {
        let m = self.m();
        if m > 1 {
            let n = end.saturating_sub(p);
            if n.checked_rem(m).unwrap_or(0) != 0 {
                let n = n.saturating_sub(n.checked_rem(m).unwrap_or(0));
                if n == 0 {
                    return None;
                }
                return Some(p.saturating_add(n));
            }
        }
        Some(end)
    }

    /// `scanComment`: `p` follows `<!-`.
    fn scan_comment(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if self.has_char(p, end) {
            if !self.matches(p, b'-') {
                return tok(INVALID, p);
            }
            p = p.saturating_add(m);
            while self.has_char(p, end) {
                match self.invalid_case(p, end) {
                    Err(t) => return t,
                    Ok(Some(n)) => {
                        p = p.saturating_add(n);
                        continue;
                    }
                    Ok(None) => {}
                }
                if self.bt(p) == Bt::Minus {
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        return bare(PARTIAL);
                    }
                    if self.matches(p, b'-') {
                        p = p.saturating_add(m);
                        if !self.has_char(p, end) {
                            return bare(PARTIAL);
                        }
                        if !self.matches(p, b'>') {
                            return tok(INVALID, p);
                        }
                        return tok(COMMENT, p.saturating_add(m));
                    }
                } else {
                    p = p.saturating_add(m);
                }
            }
        }
        bare(PARTIAL)
    }

    /// `scanDecl`: `p` follows `<!`.
    fn scan_decl(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.bt(p) {
            Bt::Minus => return self.scan_comment(p.saturating_add(m), end),
            Bt::Lsqb => return tok(COND_SECT_OPEN, p.saturating_add(m)),
            Bt::Nmstrt | Bt::Hex => p = p.saturating_add(m),
            _ => return tok(INVALID, p),
        }
        while self.has_char(p, end) {
            match self.bt(p) {
                Bt::Percnt => {
                    if !self.has_chars(p, end, 2) {
                        return bare(PARTIAL);
                    }
                    if matches!(
                        self.bt(p.saturating_add(m)),
                        Bt::S | Bt::Cr | Bt::Lf | Bt::Percnt
                    ) {
                        return tok(INVALID, p);
                    }
                    return tok(DECL_OPEN, p);
                }
                Bt::S | Bt::Cr | Bt::Lf => return tok(DECL_OPEN, p),
                Bt::Nmstrt | Bt::Hex => p = p.saturating_add(m),
                _ => return tok(INVALID, p),
            }
        }
        bare(PARTIAL)
    }

    /// `checkPiTarget`: `None` for a mixed-case `xml`.
    fn check_pi_target(&self, p: usize, end: usize) -> Option<i32> {
        let m = self.m();
        if end.saturating_sub(p) != m.saturating_mul(3) {
            return Some(PI);
        }
        let mut upper = false;
        for (i, (lo, up)) in [(b'x', b'X'), (b'm', b'M'), (b'l', b'L')]
            .into_iter()
            .enumerate()
        {
            let c = self
                .enc
                .ascii_at(self.buf, p.saturating_add(i.saturating_mul(m)));
            if c == up {
                upper = true;
            } else if c != lo {
                return Some(PI);
            }
        }
        if upper { None } else { Some(XML_DECL) }
    }

    /// `scanPi`: `p` follows `<?`.
    fn scan_pi(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        let target = p;
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.name_step(p, end, true) {
            NameStep::Ok(n) => p = p.saturating_add(n),
            NameStep::Ret(t) => return t,
            NameStep::No => return tok(INVALID, p),
        }
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            match self.bt(p) {
                Bt::S | Bt::Cr | Bt::Lf => {
                    let Some(t) = self.check_pi_target(target, p) else {
                        return tok(INVALID, p);
                    };
                    p = p.saturating_add(m);
                    while self.has_char(p, end) {
                        match self.invalid_case(p, end) {
                            Err(e) => return e,
                            Ok(Some(n)) => {
                                p = p.saturating_add(n);
                                continue;
                            }
                            Ok(None) => {}
                        }
                        if self.bt(p) == Bt::Quest {
                            p = p.saturating_add(m);
                            if !self.has_char(p, end) {
                                return bare(PARTIAL);
                            }
                            if self.matches(p, b'>') {
                                return tok(t, p.saturating_add(m));
                            }
                        } else {
                            p = p.saturating_add(m);
                        }
                    }
                    return bare(PARTIAL);
                }
                Bt::Quest => {
                    let Some(t) = self.check_pi_target(target, p) else {
                        return tok(INVALID, p);
                    };
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        return bare(PARTIAL);
                    }
                    if self.matches(p, b'>') {
                        return tok(t, p.saturating_add(m));
                    }
                    return tok(INVALID, p);
                }
                _ => return tok(INVALID, p),
            }
        }
        bare(PARTIAL)
    }

    /// `scanCdataSection`: `p` follows `<![`.
    fn scan_cdata_section(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if !self.has_chars(p, end, 6) {
            return bare(PARTIAL);
        }
        for c in *b"CDATA[" {
            if !self.matches(p, c) {
                return tok(INVALID, p);
            }
            p = p.saturating_add(m);
        }
        tok(CDATA_SECT_OPEN, p)
    }

    /// `scanEndTag`: `p` follows `</`.
    fn scan_end_tag(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.name_step(p, end, true) {
            NameStep::Ok(n) => p = p.saturating_add(n),
            NameStep::Ret(t) => return t,
            NameStep::No => return tok(INVALID, p),
        }
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            match self.bt(p) {
                Bt::S | Bt::Cr | Bt::Lf => {
                    p = p.saturating_add(m);
                    while self.has_char(p, end) {
                        match self.bt(p) {
                            Bt::S | Bt::Cr | Bt::Lf => p = p.saturating_add(m),
                            Bt::Gt => return tok(END_TAG, p.saturating_add(m)),
                            _ => return tok(INVALID, p),
                        }
                    }
                    return bare(PARTIAL);
                }
                Bt::Gt => return tok(END_TAG, p.saturating_add(m)),
                _ => return tok(INVALID, p),
            }
        }
        bare(PARTIAL)
    }

    /// `scanHexCharRef`: `p` follows `&#x`.
    fn scan_hex_char_ref(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if self.has_char(p, end) {
            if !matches!(self.bt(p), Bt::Digit | Bt::Hex) {
                return tok(INVALID, p);
            }
            p = p.saturating_add(m);
            while self.has_char(p, end) {
                match self.bt(p) {
                    Bt::Digit | Bt::Hex => {}
                    Bt::Semi => return tok(CHAR_REF, p.saturating_add(m)),
                    _ => return tok(INVALID, p),
                }
                p = p.saturating_add(m);
            }
        }
        bare(PARTIAL)
    }

    /// `scanCharRef`: `p` follows `&#`.
    fn scan_char_ref(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if self.has_char(p, end) {
            if self.matches(p, b'x') {
                return self.scan_hex_char_ref(p.saturating_add(m), end);
            }
            if self.bt(p) != Bt::Digit {
                return tok(INVALID, p);
            }
            p = p.saturating_add(m);
            while self.has_char(p, end) {
                match self.bt(p) {
                    Bt::Digit => {}
                    Bt::Semi => return tok(CHAR_REF, p.saturating_add(m)),
                    _ => return tok(INVALID, p),
                }
                p = p.saturating_add(m);
            }
        }
        bare(PARTIAL)
    }

    /// `scanRef`: `p` follows `&`.
    pub(super) fn scan_ref(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.name_step(p, end, true) {
            NameStep::Ok(n) => p = p.saturating_add(n),
            NameStep::Ret(t) => return t,
            NameStep::No => {
                if self.bt(p) == Bt::Num {
                    return self.scan_char_ref(p.saturating_add(m), end);
                }
                return tok(INVALID, p);
            }
        }
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            if self.bt(p) == Bt::Semi {
                return tok(ENTITY_REF, p.saturating_add(m));
            }
            return tok(INVALID, p);
        }
        bare(PARTIAL)
    }

    /// `scanAtts`: `p` follows the first character of the first attribute name.
    fn scan_atts(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        'name: while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            let t = self.bt(p);
            if matches!(t, Bt::S | Bt::Cr | Bt::Lf) {
                loop {
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        return bare(PARTIAL);
                    }
                    match self.bt(p) {
                        Bt::Equals => break,
                        Bt::S | Bt::Lf | Bt::Cr => {}
                        _ => return tok(INVALID, p),
                    }
                }
            } else if t != Bt::Equals {
                return tok(INVALID, p);
            }
            // BT_EQUALS
            let open;
            loop {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(PARTIAL);
                }
                let o = self.bt(p);
                if o == Bt::Quot || o == Bt::Apos {
                    open = o;
                    break;
                }
                if !matches!(o, Bt::S | Bt::Lf | Bt::Cr) {
                    return tok(INVALID, p);
                }
            }
            p = p.saturating_add(m);
            // in attribute value
            loop {
                if !self.has_char(p, end) {
                    return bare(PARTIAL);
                }
                let t = self.bt(p);
                if t == open {
                    break;
                }
                match self.invalid_case(p, end) {
                    Err(e) => return e,
                    Ok(Some(n)) => {
                        p = p.saturating_add(n);
                        continue;
                    }
                    Ok(None) => {}
                }
                match t {
                    Bt::Amp => {
                        let (rt, next) = self.scan_ref(p.saturating_add(m), end);
                        if rt <= 0 {
                            return if rt == INVALID { (rt, next) } else { bare(rt) };
                        }
                        p = next.unwrap_or(p);
                    }
                    Bt::Lt => return tok(INVALID, p),
                    _ => p = p.saturating_add(m),
                }
            }
            p = p.saturating_add(m);
            if !self.has_char(p, end) {
                return bare(PARTIAL);
            }
            match self.bt(p) {
                Bt::S | Bt::Cr | Bt::Lf => {}
                Bt::Sol => return self.sol(p, end, EMPTY_ELEMENT_WITH_ATTS),
                Bt::Gt => return tok(START_TAG_WITH_ATTS, p.saturating_add(m)),
                _ => return tok(INVALID, p),
            }
            // p points to the closing quote's successor whitespace
            loop {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(PARTIAL);
                }
                match self.name_step(p, end, true) {
                    NameStep::Ok(n) => {
                        p = p.saturating_add(n);
                        continue 'name;
                    }
                    NameStep::Ret(t) => return t,
                    NameStep::No => {}
                }
                match self.bt(p) {
                    Bt::S | Bt::Cr | Bt::Lf => {}
                    Bt::Gt => return tok(START_TAG_WITH_ATTS, p.saturating_add(m)),
                    Bt::Sol => return self.sol(p, end, EMPTY_ELEMENT_WITH_ATTS),
                    _ => return tok(INVALID, p),
                }
            }
        }
        bare(PARTIAL)
    }

    /// The `sol:` label: `/` must be followed by `>`.
    fn sol(&self, mut p: usize, end: usize, t: i32) -> Tok {
        let m = self.m();
        p = p.saturating_add(m);
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        if !self.matches(p, b'>') {
            return tok(INVALID, p);
        }
        tok(t, p.saturating_add(m))
    }

    /// `scanLt`: `p` follows `<`.
    fn scan_lt(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.name_step(p, end, true) {
            NameStep::Ok(n) => p = p.saturating_add(n),
            NameStep::Ret(t) => return t,
            NameStep::No => match self.bt(p) {
                Bt::Excl => {
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        return bare(PARTIAL);
                    }
                    return match self.bt(p) {
                        Bt::Minus => self.scan_comment(p.saturating_add(m), end),
                        Bt::Lsqb => self.scan_cdata_section(p.saturating_add(m), end),
                        _ => tok(INVALID, p),
                    };
                }
                Bt::Quest => return self.scan_pi(p.saturating_add(m), end),
                Bt::Sol => return self.scan_end_tag(p.saturating_add(m), end),
                _ => return tok(INVALID, p),
            },
        }
        // a start tag
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            match self.bt(p) {
                Bt::S | Bt::Cr | Bt::Lf => {
                    p = p.saturating_add(m);
                    while self.has_char(p, end) {
                        match self.name_step(p, end, true) {
                            NameStep::Ok(n) => return self.scan_atts(p.saturating_add(n), end),
                            NameStep::Ret(t) => return t,
                            NameStep::No => {}
                        }
                        match self.bt(p) {
                            Bt::Gt => return tok(START_TAG_NO_ATTS, p.saturating_add(m)),
                            Bt::Sol => return self.sol(p, end, EMPTY_ELEMENT_NO_ATTS),
                            Bt::S | Bt::Cr | Bt::Lf => p = p.saturating_add(m),
                            _ => return tok(INVALID, p),
                        }
                    }
                    return bare(PARTIAL);
                }
                Bt::Gt => return tok(START_TAG_NO_ATTS, p.saturating_add(m)),
                Bt::Sol => return self.sol(p, end, EMPTY_ELEMENT_NO_ATTS),
                _ => return tok(INVALID, p),
            }
        }
        bare(PARTIAL)
    }

    /// `contentTok`.
    pub(super) fn content_tok(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if p >= end {
            return bare(NONE);
        }
        let Some(end) = self.align(p, end) else {
            return bare(PARTIAL);
        };
        match self.bt(p) {
            Bt::Lt => return self.scan_lt(p.saturating_add(m), end),
            Bt::Amp => return self.scan_ref(p.saturating_add(m), end),
            Bt::Cr => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(TRAILING_CR);
                }
                if self.bt(p) == Bt::Lf {
                    p = p.saturating_add(m);
                }
                return tok(DATA_NEWLINE, p);
            }
            Bt::Lf => return tok(DATA_NEWLINE, p.saturating_add(m)),
            Bt::Rsqb => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(TRAILING_RSQB);
                }
                if self.matches(p, b']') {
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        return bare(TRAILING_RSQB);
                    }
                    if self.matches(p, b'>') {
                        return tok(INVALID, p);
                    }
                    p = p.saturating_sub(m);
                }
            }
            _ => match self.invalid_case(p, end) {
                Err(e) => return e,
                Ok(Some(n)) => p = p.saturating_add(n),
                Ok(None) => p = p.saturating_add(m),
            },
        }
        while self.has_char(p, end) {
            match self.bt(p) {
                t @ (Bt::Lead2 | Bt::Lead3 | Bt::Lead4) => {
                    let n = match t {
                        Bt::Lead2 => 2,
                        Bt::Lead3 => 3,
                        _ => 4,
                    };
                    if end.saturating_sub(p) < n || self.enc.is_invalid_char(self.buf, p, n) {
                        return tok(DATA_CHARS, p);
                    }
                    p = p.saturating_add(n);
                }
                Bt::Rsqb => {
                    if self.has_chars(p, end, 2) {
                        if !self.matches(p.saturating_add(m), b']') {
                            p = p.saturating_add(m);
                            continue;
                        }
                        if self.has_chars(p, end, 3) {
                            if !self.matches(p.saturating_add(2usize.saturating_mul(m)), b'>') {
                                p = p.saturating_add(m);
                                continue;
                            }
                            return tok(INVALID, p.saturating_add(2usize.saturating_mul(m)));
                        }
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::Amp | Bt::Lt | Bt::Nonxml | Bt::Malform | Bt::Trail | Bt::Cr | Bt::Lf => {
                    return tok(DATA_CHARS, p);
                }
                _ => p = p.saturating_add(m),
            }
        }
        tok(DATA_CHARS, p)
    }

    /// `cdataSectionTok`.
    pub(super) fn cdata_section_tok(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if p >= end {
            return bare(NONE);
        }
        let Some(end) = self.align(p, end) else {
            return bare(PARTIAL);
        };
        match self.bt(p) {
            Bt::Rsqb => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(PARTIAL);
                }
                if self.matches(p, b']') {
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        return bare(PARTIAL);
                    }
                    if self.matches(p, b'>') {
                        return tok(CDATA_SECT_CLOSE, p.saturating_add(m));
                    }
                    p = p.saturating_sub(m);
                }
            }
            Bt::Cr => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(PARTIAL);
                }
                if self.bt(p) == Bt::Lf {
                    p = p.saturating_add(m);
                }
                return tok(DATA_NEWLINE, p);
            }
            Bt::Lf => return tok(DATA_NEWLINE, p.saturating_add(m)),
            _ => match self.invalid_case(p, end) {
                Err(e) => return e,
                Ok(Some(n)) => p = p.saturating_add(n),
                Ok(None) => p = p.saturating_add(m),
            },
        }
        while self.has_char(p, end) {
            match self.bt(p) {
                t @ (Bt::Lead2 | Bt::Lead3 | Bt::Lead4) => {
                    let n = match t {
                        Bt::Lead2 => 2,
                        Bt::Lead3 => 3,
                        _ => 4,
                    };
                    if end.saturating_sub(p) < n || self.enc.is_invalid_char(self.buf, p, n) {
                        return tok(DATA_CHARS, p);
                    }
                    p = p.saturating_add(n);
                }
                Bt::Nonxml | Bt::Malform | Bt::Trail | Bt::Cr | Bt::Lf | Bt::Rsqb => {
                    return tok(DATA_CHARS, p);
                }
                _ => p = p.saturating_add(m),
            }
        }
        tok(DATA_CHARS, p)
    }

    /// `scanPercent`: `p` follows `%`.
    fn scan_percent(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.name_step(p, end, true) {
            NameStep::Ok(n) => p = p.saturating_add(n),
            NameStep::Ret(t) => return t,
            NameStep::No => {
                return match self.bt(p) {
                    Bt::S | Bt::Lf | Bt::Cr | Bt::Percnt => tok(PERCENT, p),
                    _ => tok(INVALID, p),
                };
            }
        }
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            if self.bt(p) == Bt::Semi {
                return tok(PARAM_ENTITY_REF, p.saturating_add(m));
            }
            return tok(INVALID, p);
        }
        bare(PARTIAL)
    }

    /// `scanPoundName`: `p` follows `#`.
    fn scan_pound_name(&self, mut p: usize, end: usize) -> Tok {
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        match self.name_step(p, end, true) {
            NameStep::Ok(n) => p = p.saturating_add(n),
            NameStep::Ret(t) => return t,
            NameStep::No => return tok(INVALID, p),
        }
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(t) => return t,
                NameStep::No => {}
            }
            return match self.bt(p) {
                Bt::Cr | Bt::Lf | Bt::S | Bt::Rpar | Bt::Gt | Bt::Percnt | Bt::Verbar => {
                    tok(POUND_NAME, p)
                }
                _ => tok(INVALID, p),
            };
        }
        bare(-POUND_NAME)
    }

    /// `scanLit`: `p` follows the opening quote.
    fn scan_lit(&self, open: Bt, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        while self.has_char(p, end) {
            match self.invalid_case(p, end) {
                Err(e) => return e,
                Ok(Some(n)) => {
                    p = p.saturating_add(n);
                    continue;
                }
                Ok(None) => {}
            }
            let t = self.bt(p);
            if t == Bt::Quot || t == Bt::Apos {
                p = p.saturating_add(m);
                if t != open {
                    continue;
                }
                if !self.has_char(p, end) {
                    return bare(-LITERAL);
                }
                return match self.bt(p) {
                    Bt::S | Bt::Cr | Bt::Lf | Bt::Gt | Bt::Percnt | Bt::Lsqb => tok(LITERAL, p),
                    _ => tok(INVALID, p),
                };
            }
            p = p.saturating_add(m);
        }
        bare(PARTIAL)
    }

    /// `prologTok`.
    pub(super) fn prolog_tok(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if p >= end {
            return bare(NONE);
        }
        let Some(end) = self.align(p, end) else {
            return bare(PARTIAL);
        };
        let mut t;
        match self.bt(p) {
            Bt::Quot => return self.scan_lit(Bt::Quot, p.saturating_add(m), end),
            Bt::Apos => return self.scan_lit(Bt::Apos, p.saturating_add(m), end),
            Bt::Lt => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(PARTIAL);
                }
                return match self.bt(p) {
                    Bt::Excl => self.scan_decl(p.saturating_add(m), end),
                    Bt::Quest => self.scan_pi(p.saturating_add(m), end),
                    Bt::Nmstrt | Bt::Hex | Bt::Nonascii | Bt::Lead2 | Bt::Lead3 | Bt::Lead4 => {
                        tok(INSTANCE_START, p.saturating_sub(m))
                    }
                    _ => tok(INVALID, p),
                };
            }
            b @ (Bt::Cr | Bt::S | Bt::Lf) => {
                if b == Bt::Cr && p.saturating_add(m) == end {
                    return tok(-PROLOG_S, end);
                }
                loop {
                    p = p.saturating_add(m);
                    if !self.has_char(p, end) {
                        break;
                    }
                    match self.bt(p) {
                        Bt::S | Bt::Lf => {}
                        Bt::Cr if p.saturating_add(m) != end => {}
                        _ => return tok(PROLOG_S, p),
                    }
                }
                return tok(PROLOG_S, p);
            }
            Bt::Percnt => return self.scan_percent(p.saturating_add(m), end),
            Bt::Comma => return tok(COMMA, p.saturating_add(m)),
            Bt::Lsqb => return tok(OPEN_BRACKET, p.saturating_add(m)),
            Bt::Rsqb => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(-CLOSE_BRACKET);
                }
                if self.matches(p, b']') {
                    if !self.has_chars(p, end, 2) {
                        return bare(PARTIAL);
                    }
                    if self.matches(p.saturating_add(m), b'>') {
                        return tok(COND_SECT_CLOSE, p.saturating_add(2usize.saturating_mul(m)));
                    }
                }
                return tok(CLOSE_BRACKET, p);
            }
            Bt::Lpar => return tok(OPEN_PAREN, p.saturating_add(m)),
            Bt::Rpar => {
                p = p.saturating_add(m);
                if !self.has_char(p, end) {
                    return bare(-CLOSE_PAREN);
                }
                return match self.bt(p) {
                    Bt::Ast => tok(CLOSE_PAREN_ASTERISK, p.saturating_add(m)),
                    Bt::Quest => tok(CLOSE_PAREN_QUESTION, p.saturating_add(m)),
                    Bt::Plus => tok(CLOSE_PAREN_PLUS, p.saturating_add(m)),
                    Bt::Cr | Bt::Lf | Bt::S | Bt::Gt | Bt::Comma | Bt::Verbar | Bt::Rpar => {
                        tok(CLOSE_PAREN, p)
                    }
                    _ => tok(INVALID, p),
                };
            }
            Bt::Verbar => return tok(OR, p.saturating_add(m)),
            Bt::Gt => return tok(DECL_CLOSE, p.saturating_add(m)),
            Bt::Num => return self.scan_pound_name(p.saturating_add(m), end),
            lt @ (Bt::Lead2 | Bt::Lead3 | Bt::Lead4) => {
                let n = match lt {
                    Bt::Lead2 => 2,
                    Bt::Lead3 => 3,
                    _ => 4,
                };
                if end.saturating_sub(p) < n {
                    return bare(PARTIAL_CHAR);
                }
                if self.enc.is_invalid_char(self.buf, p, n) {
                    return tok(INVALID, p);
                }
                if self.enc.is_nmstrt_char(self.buf, p, n) {
                    p = p.saturating_add(n);
                    t = NAME;
                } else if self.enc.is_name_char(self.buf, p, n) {
                    p = p.saturating_add(n);
                    t = NMTOKEN;
                } else {
                    return tok(INVALID, p);
                }
            }
            Bt::Nmstrt | Bt::Hex => {
                t = NAME;
                p = p.saturating_add(m);
            }
            Bt::Digit | Bt::Name | Bt::Minus => {
                t = NMTOKEN;
                p = p.saturating_add(m);
            }
            Bt::Nonascii => {
                if self.enc.is_nmstrt_char_minbpc(self.buf, p) {
                    p = p.saturating_add(m);
                    t = NAME;
                } else if self.enc.is_name_char_minbpc(self.buf, p) {
                    p = p.saturating_add(m);
                    t = NMTOKEN;
                } else {
                    return tok(INVALID, p);
                }
            }
            _ => return tok(INVALID, p),
        }
        while self.has_char(p, end) {
            match self.name_step(p, end, false) {
                NameStep::Ok(n) => {
                    p = p.saturating_add(n);
                    continue;
                }
                NameStep::Ret(r) => return r,
                NameStep::No => {}
            }
            match self.bt(p) {
                Bt::Gt
                | Bt::Rpar
                | Bt::Comma
                | Bt::Verbar
                | Bt::Lsqb
                | Bt::Percnt
                | Bt::S
                | Bt::Cr
                | Bt::Lf => {
                    return tok(t, p);
                }
                b @ (Bt::Plus | Bt::Ast | Bt::Quest) => {
                    if t == NMTOKEN {
                        return tok(INVALID, p);
                    }
                    t = match b {
                        Bt::Plus => NAME_PLUS,
                        Bt::Ast => NAME_ASTERISK,
                        _ => NAME_QUESTION,
                    };
                    return tok(t, p.saturating_add(m));
                }
                _ => return tok(INVALID, p),
            }
        }
        bare(t.saturating_neg())
    }

    /// `attributeValueTok`.
    pub(super) fn attribute_value_tok(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if p >= end {
            return bare(NONE);
        }
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        let start = p;
        while self.has_char(p, end) {
            match self.bt(p) {
                Bt::Lead2 => p = p.saturating_add(2),
                Bt::Lead3 => p = p.saturating_add(3),
                Bt::Lead4 => p = p.saturating_add(4),
                Bt::Amp => {
                    if p == start {
                        return self.scan_ref(p.saturating_add(m), end);
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::Lt => return tok(INVALID, p),
                Bt::Lf => {
                    if p == start {
                        return tok(DATA_NEWLINE, p.saturating_add(m));
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::Cr => {
                    if p == start {
                        p = p.saturating_add(m);
                        if !self.has_char(p, end) {
                            return bare(TRAILING_CR);
                        }
                        if self.bt(p) == Bt::Lf {
                            p = p.saturating_add(m);
                        }
                        return tok(DATA_NEWLINE, p);
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::S => {
                    if p == start {
                        return tok(ATTRIBUTE_VALUE_S, p.saturating_add(m));
                    }
                    return tok(DATA_CHARS, p);
                }
                _ => p = p.saturating_add(m),
            }
        }
        tok(DATA_CHARS, p)
    }

    /// `entityValueTok`.
    pub(super) fn entity_value_tok(&self, mut p: usize, end: usize) -> Tok {
        let m = self.m();
        if p >= end {
            return bare(NONE);
        }
        if !self.has_char(p, end) {
            return bare(PARTIAL);
        }
        let start = p;
        while self.has_char(p, end) {
            match self.bt(p) {
                Bt::Lead2 => p = p.saturating_add(2),
                Bt::Lead3 => p = p.saturating_add(3),
                Bt::Lead4 => p = p.saturating_add(4),
                Bt::Amp => {
                    if p == start {
                        return self.scan_ref(p.saturating_add(m), end);
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::Percnt => {
                    if p == start {
                        let r = self.scan_percent(p.saturating_add(m), end);
                        return if r.0 == PERCENT { (INVALID, r.1) } else { r };
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::Lf => {
                    if p == start {
                        return tok(DATA_NEWLINE, p.saturating_add(m));
                    }
                    return tok(DATA_CHARS, p);
                }
                Bt::Cr => {
                    if p == start {
                        p = p.saturating_add(m);
                        if !self.has_char(p, end) {
                            return bare(TRAILING_CR);
                        }
                        if self.bt(p) == Bt::Lf {
                            p = p.saturating_add(m);
                        }
                        return tok(DATA_NEWLINE, p);
                    }
                    return tok(DATA_CHARS, p);
                }
                _ => p = p.saturating_add(m),
            }
        }
        tok(DATA_CHARS, p)
    }

    /// `isPublicId`: `Err(bad)` at the first disallowed character of the literal `p..end` (quotes included).
    pub(super) fn is_public_id(&self, p: usize, end: usize) -> Result<(), usize> {
        let m = self.m();
        let mut q = p.saturating_add(m);
        let end = end.saturating_sub(m);
        while self.has_char(q, end) {
            let ok = match self.bt(q) {
                Bt::Digit
                | Bt::Hex
                | Bt::Minus
                | Bt::Apos
                | Bt::Lpar
                | Bt::Rpar
                | Bt::Plus
                | Bt::Comma
                | Bt::Sol
                | Bt::Equals
                | Bt::Quest
                | Bt::Cr
                | Bt::Lf
                | Bt::Semi
                | Bt::Excl
                | Bt::Ast
                | Bt::Percnt
                | Bt::Num => true,
                Bt::S => !self.matches(q, b'\t'),
                b => {
                    let raw = self.raw_ascii(q);
                    if matches!(b, Bt::Name | Bt::Nmstrt) && raw >= 0 && raw & !0x7f == 0 {
                        true
                    } else {
                        raw == 0x24 || raw == 0x40
                    }
                }
            };
            if !ok {
                return Err(q);
            }
            q = q.saturating_add(m);
        }
        Ok(())
    }

    /// `BYTE_TO_ASCII(enc, p)` as expat computes it (the raw byte for 8-bit encodings, −1 off the ASCII range
    /// of UTF-16).
    fn raw_ascii(&self, p: usize) -> i32 {
        let b0 = i32::from(self.buf.get(p).copied().unwrap_or(0));
        let b1 = i32::from(self.buf.get(p.saturating_add(1)).copied().unwrap_or(0));
        match self.enc {
            Enc::Little2 => {
                if b1 == 0 {
                    b0
                } else {
                    -1
                }
            }
            Enc::Big2 => {
                if b0 == 0 {
                    b1
                } else {
                    -1
                }
            }
            _ => b0,
        }
    }

    /// `nameLength`.
    pub(super) fn name_length(&self, start: usize) -> usize {
        let mut p = start;
        loop {
            match self.bt(p) {
                Bt::Lead2 => p = p.saturating_add(2),
                Bt::Lead3 => p = p.saturating_add(3),
                Bt::Lead4 => p = p.saturating_add(4),
                Bt::Nonascii | Bt::Nmstrt | Bt::Hex | Bt::Digit | Bt::Name | Bt::Minus => {
                    p = p.saturating_add(self.m());
                }
                _ => return p.saturating_sub(start),
            }
            if p >= self.buf.len() {
                return p.min(self.buf.len()).saturating_sub(start);
            }
        }
    }

    /// `nameMatchesAscii(ptr1, end1, kw)`.
    pub(super) fn name_matches_ascii(&self, mut p: usize, end: usize, kw: &[u8]) -> bool {
        let m = self.m();
        for &c in kw {
            if end.saturating_sub(p) < m || p > end {
                return false;
            }
            if !self.matches(p, c) {
                return false;
            }
            p = p.saturating_add(m);
        }
        p == end
    }

    /// `charRefNumber` (`p` at `&#`).
    #[expect(
        clippy::many_single_char_names,
        reason = "the names of expat's charRefNumber"
    )]
    pub(super) fn char_ref_number(&self, p: usize) -> i64 {
        let m = self.m();
        let mut q = p.saturating_add(2usize.saturating_mul(m));
        let mut result: i64 = 0;
        if self.matches(q, b'x') {
            q = q.saturating_add(m);
            while !self.matches(q, b';') && q < self.buf.len() {
                let c = self.enc.ascii_at(self.buf, q);
                let d = match c {
                    b'0'..=b'9' => Some(i64::from(c.wrapping_sub(b'0'))),
                    b'A'..=b'F' => Some(i64::from(c.wrapping_sub(b'A')).saturating_add(10)),
                    b'a'..=b'f' => Some(i64::from(c.wrapping_sub(b'a')).saturating_add(10)),
                    _ => None,
                };
                if let Some(d) = d {
                    result = (result << 4) | d;
                }
                if result >= 0x11_0000 {
                    return -1;
                }
                q = q.saturating_add(m);
            }
        } else {
            while !self.matches(q, b';') && q < self.buf.len() {
                let c = i64::from(self.enc.ascii_at(self.buf, q));
                result = result
                    .saturating_mul(10)
                    .saturating_add(c.saturating_sub(i64::from(b'0')));
                if result >= 0x11_0000 {
                    return -1;
                }
                q = q.saturating_add(m);
            }
        }
        super::enc::check_char_ref_number(result)
    }

    /// `predefinedEntityName(ptr, end)`: the character of `lt`/`gt`/`amp`/`quot`/`apos`.
    pub(super) fn predefined_entity_name(&self, p: usize, end: usize) -> Option<u8> {
        for (name, c) in [
            (&b"lt"[..], b'<'),
            (b"gt", b'>'),
            (b"amp", b'&'),
            (b"quot", b'"'),
            (b"apos", b'\''),
        ] {
            if self.name_matches_ascii(p, end, name) {
                return Some(c);
            }
        }
        None
    }

    /// `getAtts`: each attribute's name start, value range and whether its value is already normalized.
    pub(super) fn get_atts(&self, start: usize) -> Vec<Att> {
        #[derive(PartialEq, Eq)]
        enum State {
            Other,
            InName,
            InValue,
        }
        let m = self.m();
        let mut state = State::InName;
        let mut atts: Vec<Att> = Vec::new();
        let mut cur = Att::default();
        let mut open = Bt::Quot;
        let mut p = start.saturating_add(m);
        while p < self.buf.len() {
            let t = self.bt(p);
            match t {
                Bt::Lead2 | Bt::Lead3 | Bt::Lead4 | Bt::Nonascii | Bt::Nmstrt | Bt::Hex => {
                    if state == State::Other {
                        cur = Att {
                            name: p,
                            normalized: true,
                            ..Att::default()
                        };
                        state = State::InName;
                    }
                    match t {
                        Bt::Lead2 => p = p.saturating_add(2usize.saturating_sub(m.min(2))),
                        Bt::Lead3 => p = p.saturating_add(3usize.saturating_sub(m.min(3))),
                        Bt::Lead4 => p = p.saturating_add(4usize.saturating_sub(m.min(4))),
                        _ => {}
                    }
                }
                Bt::Quot | Bt::Apos => {
                    if state != State::InValue {
                        cur.value = p.saturating_add(m);
                        state = State::InValue;
                        open = t;
                    } else if open == t {
                        state = State::Other;
                        cur.value_end = p;
                        atts.push(cur.clone());
                    }
                }
                Bt::Amp => cur.normalized = false,
                Bt::S => {
                    if state == State::InName {
                        state = State::Other;
                    } else if state == State::InValue
                        && cur.normalized
                        && (p == cur.value
                            || self.enc.ascii_at(self.buf, p) != b' '
                            || self.enc.ascii_at(self.buf, p.saturating_add(m)) == b' '
                            || self.bt(p.saturating_add(m)) == open)
                    {
                        cur.normalized = false;
                    }
                }
                Bt::Cr | Bt::Lf => {
                    if state == State::InName {
                        state = State::Other;
                    } else if state == State::InValue {
                        cur.normalized = false;
                    }
                }
                Bt::Gt | Bt::Sol if state != State::InValue => return atts,
                _ => {}
            }
            p = p.saturating_add(m);
        }
        atts
    }
}

/// An attribute of a start tag (`ATTRIBUTE`).
#[derive(Clone, Debug, Default)]
pub(super) struct Att {
    pub(super) name: usize,
    pub(super) value: usize,
    pub(super) value_end: usize,
    pub(super) normalized: bool,
}
