//! A minimal JSON reader (RFC 8259) for the committed vector files, with a `serde_json::Value`-like
//! surface (`v["key"][0]`, `as_str`, …; missing keys index to `Null`).

use std::fmt;
use std::ops::Index;

/// A JSON value. Numbers keep their source text so that integers beyond `i64` stay exact.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

static NULL: Value = Value::Null;

impl Value {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
        };
        let v = p.value()?;
        p.ws();
        if p.i == p.s.len() {
            Ok(v)
        } else {
            Err(format!("trailing data at {}", p.i))
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&Vec<Self>> {
        match self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    pub(crate) fn as_object(&self) -> Option<&Vec<(String, Self)>> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }

    pub(crate) fn as_i128(&self) -> Option<i128> {
        match self {
            Self::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub(crate) fn as_i64(&self) -> Option<i64> {
        self.as_i128().and_then(|n| i64::try_from(n).ok())
    }

    pub(crate) fn as_u64(&self) -> Option<u64> {
        self.as_i128().and_then(|n| u64::try_from(n).ok())
    }
}

impl Index<&str> for Value {
    type Output = Self;
    fn index(&self, index: &str) -> &Self {
        self.get(index).unwrap_or(&NULL)
    }
}

impl Index<usize> for Value {
    type Output = Self;
    fn index(&self, index: usize) -> &Self {
        self.as_array().and_then(|a| a.get(index)).unwrap_or(&NULL)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Number(n) => f.write_str(n),
            Self::String(s) => write!(f, "{s:?}"),
            Self::Array(a) => {
                f.write_str("[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Self::Object(o) => {
                f.write_str("{")?;
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{k:?}:{v}")?;
                }
                f.write_str("}")
            }
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn expect(&mut self, lit: &str) -> Result<(), String> {
        if self.s.get(self.i..self.i + lit.len()) == Some(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(format!("expected {lit} at {}", self.i))
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.ws();
        match self.peek() {
            Some(b'n') => self.expect("null").map(|()| Value::Null),
            Some(b't') => self.expect("true").map(|()| Value::Bool(true)),
            Some(b'f') => self.expect("false").map(|()| Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => {
                self.i += 1;
                let mut out = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Value::Array(out));
                }
                loop {
                    out.push(self.value()?);
                    self.ws();
                    match self.bump() {
                        Some(b',') => {}
                        Some(b']') => return Ok(Value::Array(out)),
                        _ => return Err(format!("bad array at {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut out = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Value::Object(out));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.expect(":")?;
                    let v = self.value()?;
                    out.push((k, v));
                    self.ws();
                    match self.bump() {
                        Some(b',') => {}
                        Some(b'}') => return Ok(Value::Object(out)),
                        _ => return Err(format!("bad object at {}", self.i)),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while matches!(
                    self.peek(),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.i += 1;
                }
                let text = std::str::from_utf8(self.s.get(start..self.i).unwrap_or_default())
                    .map_err(|e| e.to_string())?;
                Ok(Value::Number(text.to_owned()))
            }
            _ => Err(format!("unexpected input at {}", self.i)),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).map_err(|e| e.to_string())?, 16)
            .map_err(|e| e.to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect("\"")?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.bump().ok_or("unterminated string")? {
                b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let c = match self.bump().ok_or("bad escape")? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xd800..0xdc00).contains(&hi)
                                && self.s.get(self.i..self.i + 2) == Some(b"\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                                } else {
                                    self.i = save;
                                    0xfffd
                                }
                            } else {
                                hi
                            };
                            char::from_u32(cp).unwrap_or('\u{fffd}')
                        }
                        other => return Err(format!("bad escape {other}")),
                    };
                    let mut buf = [0_u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                b => out.push(b),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Value;

    #[test]
    fn parses_vectors_shapes() {
        let v = Value::parse(
            r#"{"a": [1, -2.5e3, 100000000000000000000], "s": "xé😀\n", "n": null, "t": true}"#,
        )
        .unwrap();
        assert_eq!(v["a"][0].as_i64(), Some(1));
        assert_eq!(v["a"][2].as_i128(), Some(100_000_000_000_000_000_000));
        assert_eq!(v["s"].as_str(), Some("xé😀\n"));
        assert_eq!(v["n"], Value::Null);
        assert_eq!(v["missing"], Value::Null);
        assert_eq!(v["t"].as_bool(), Some(true));
    }
}
