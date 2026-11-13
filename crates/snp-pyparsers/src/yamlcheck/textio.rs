//! `open(filename, encoding='UTF-8')` as ruamel's reader consumes it: CPython's `TextIOWrapper.read(n)` over
//! 8192-byte binary chunks, an incremental strict UTF-8 decoder and the universal-newline decoder
//! (`IncrementalNewlineDecoder(translate=True)`). Decoding is lazy, so an invalid byte only raises
//! `UnicodeDecodeError` when the reader pulls the chunk that holds it, and the error's positions are relative
//! to that decoder call's input (the held-back bytes plus the chunk), exactly as in Python.
//!
//! Owner: WP-17.

use super::{Fail, R};

/// `TextIOWrapper._CHUNK_SIZE`.
const CHUNK: usize = 8192;

/// The text stream of one file.
#[derive(Debug)]
pub(super) struct TextIo<'a> {
    bytes: &'a [u8],
    /// Next raw byte to read.
    pos: usize,
    /// Bytes of an incomplete UTF-8 sequence held by the incremental decoder.
    pending: Vec<u8>,
    /// A `\r` held back by the newline decoder (it may be the first half of `\r\n`).
    pendingcr: bool,
    /// Decoded characters not yet returned.
    decoded: Vec<char>,
    dpos: usize,
}

impl<'a> TextIo<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            pending: Vec::new(),
            pendingcr: false,
            decoded: Vec::new(),
            dpos: 0,
        }
    }

    /// `read(n)`: up to `n` characters, fewer only at EOF (`textiowrapper_read_impl`: characters the final decode
    /// produces at EOF are returned by the next call). `before_decode` runs before the first chunk is decoded:
    /// decoding runs the Python-level `getstate`/`decode` methods of the incremental decoder, one frame below
    /// the caller, so this is where the caller checks its recursion headroom.
    pub(super) fn read(
        &mut self,
        n: usize,
        mut before_decode: impl FnMut() -> R<()>,
    ) -> R<Vec<char>> {
        let mut out = Vec::new();
        self.take_decoded(&mut out, n);
        let mut first = true;
        while out.len() < n {
            if first {
                before_decode()?;
                first = false;
            }
            if !self.read_chunk()? {
                break;
            }
            self.take_decoded(&mut out, n);
        }
        Ok(out)
    }

    /// Moves decoded characters into `out` until it holds `n`.
    fn take_decoded(&mut self, out: &mut Vec<char>, n: usize) {
        let want = n.saturating_sub(out.len());
        let end = self.dpos.saturating_add(want).min(self.decoded.len());
        if let Some(s) = self.decoded.get(self.dpos..end) {
            out.extend_from_slice(s);
        }
        self.dpos = end.max(self.dpos);
    }

    /// `textiowrapper_read_chunk`: one raw read of 8192 bytes and its decode; `false` at EOF when the final decode
    /// produced nothing.
    fn read_chunk(&mut self) -> R<bool> {
        let end = self.pos.saturating_add(CHUNK).min(self.bytes.len());
        let chunk = self.bytes.get(self.pos..end).unwrap_or_default();
        self.pos = end;
        let eof = chunk.is_empty();
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        let text: String = match std::str::from_utf8(&data) {
            Ok(s) => s.to_owned(),
            Err(e) if e.error_len().is_none() && !eof => {
                let valid = e.valid_up_to();
                let head = std::str::from_utf8(data.get(..valid).unwrap_or_default())
                    .unwrap_or_default()
                    .to_owned();
                self.pending = data.get(valid..).unwrap_or_default().to_vec();
                head
            }
            Err(_) => {
                let err = snp_pycompat::codec::decode_utf8(&data)
                    .err()
                    .map_or_default(|e| e.to_string());
                return Err(Fail::crash("UnicodeDecodeError", err));
            }
        };
        let mut s = String::new();
        if self.pendingcr && (!text.is_empty() || eof) {
            s.push('\r');
            self.pendingcr = false;
        }
        s.push_str(&text);
        if !eof && s.ends_with('\r') {
            s.pop();
            self.pendingcr = true;
        }
        let mut chars = Vec::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\r' {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                chars.push('\n');
            } else {
                chars.push(c);
            }
        }
        // `if (nchars > 0) eof = 0;`: the final decode can still produce characters (a held-back `\r`), and
        // they are returned by this read, not lost.
        let more = !eof || !chars.is_empty();
        self.decoded = chars;
        self.dpos = 0;
        Ok(more)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(b: &[u8], n: usize) -> R<String> {
        let mut t = TextIo::new(b);
        let mut out = String::new();
        loop {
            let got = t.read(n, || Ok(()))?;
            if got.is_empty() {
                return Ok(out);
            }
            out.extend(got);
        }
    }

    #[test]
    fn newlines_and_chunks() {
        assert_eq!(
            read_all(b"a\r\nb\rc\n", 2).ok().as_deref(),
            Some("a\nb\nc\n")
        );
        let mut big = vec![b'x'; 8191];
        big.extend_from_slice(b"\r\ny");
        assert_eq!(read_all(&big, 4096).ok().map(|s| s.len()), Some(8191 + 2));
        let mut euro = vec![b'x'; 8191];
        euro.extend_from_slice("€".as_bytes());
        assert_eq!(
            read_all(&euro, 4096).ok().map(|s| s.chars().count()),
            Some(8192)
        );
    }

    #[test]
    fn trailing_cr_at_a_read_boundary_is_kept() {
        // `read(4096)` satisfied exactly, then EOF with the held-back `\r`: CPython returns it as `\n`.
        for len in [4097_usize, 8193, 65537] {
            let mut b = vec![b'x'; len - 1];
            b.push(b'\r');
            let mut t = TextIo::new(&b);
            let mut out = String::new();
            loop {
                let got = t.read(4096, || Ok(())).ok().unwrap_or_default();
                if got.is_empty() {
                    break;
                }
                out.extend(got);
            }
            assert_eq!(out.len(), len, "{len}");
            assert!(out.ends_with("x\n"), "{len}");
        }
    }

    #[test]
    fn decode_errors_are_chunk_relative() {
        let mut b = vec![b'x'; 8195];
        b.push(0xff);
        match read_all(&b, 4096) {
            Err(Fail::Crash(ty, text)) => {
                assert_eq!(ty, "UnicodeDecodeError");
                assert_eq!(
                    text,
                    "'utf-8' codec can't decode byte 0xff in position 3: invalid start byte"
                );
            }
            other => panic!("{other:?}"),
        }
        match read_all(b"ab\xe2\x82", 4096) {
            Err(Fail::Crash(_, text)) => assert_eq!(
                text,
                "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data"
            ),
            other => panic!("{other:?}"),
        }
    }
}
