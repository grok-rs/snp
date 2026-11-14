//! Text / binary detection: `identify.is_text`.

use std::io::{self, Read};

/// Number of leading bytes identify inspects to decide `text` vs `binary`.
pub const TEXT_PROBE_LEN: usize = 1024;

/// Whether `b` is a "text" byte for identify (and libmagic): BEL, BS, TAB,
/// LF, VT, FF, CR, ESC, printable ASCII, and every byte `>= 0x80`.
const fn is_text_byte(b: u8) -> bool {
    matches!(b, 7..=13 | 27 | 0x20..=0x7e | 0x80..=0xff)
}

/// Whether the first [`TEXT_PROBE_LEN`] bytes of `content` look like text,
/// exactly like `identify.is_text(io.BytesIO(content))`.
///
/// Bytes after the first 1024 are ignored. Empty content is text.
///
/// ```
/// use file_identify::is_text_bytes;
///
/// assert!(is_text_bytes("héllo\tworld\n".as_bytes()));
/// assert!(!is_text_bytes(b"\x7fELF\x02\x01\x01\x00"));
/// assert!(is_text_bytes(b""));
/// ```
#[must_use]
pub fn is_text_bytes(content: &[u8]) -> bool {
    let head = content.get(..TEXT_PROBE_LEN).unwrap_or(content);
    // Branch-free inner loop so the compiler can vectorize it.
    #[expect(clippy::disallowed_methods, reason = "a constant non-zero chunk size")]
    let mut chunks = head.chunks(64);
    chunks.all(|chunk| chunk.iter().fold(true, |ok, &b| ok & is_text_byte(b)))
}

/// Read up to [`TEXT_PROBE_LEN`] bytes from `reader` and report whether they
/// look like text, exactly like `identify.is_text`.
///
/// Like Python's buffered `read(1024)`, short reads are retried until 1024
/// bytes or end of input; [`io::ErrorKind::Interrupted`] is retried.
///
/// # Errors
///
/// Propagates read errors.
///
/// ```
/// use file_identify::is_text;
///
/// assert!(is_text(&b"plain text"[..])?);
/// assert!(!is_text(std::io::Cursor::new([0x7f, 0x45, 0x4c, 0x46, 0x00]))?);
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn is_text<R: Read>(mut reader: R) -> io::Result<bool> {
    let mut buf = [0u8; TEXT_PROBE_LEN];
    let n = read_full(&mut reader, &mut buf)?;
    Ok(is_text_bytes(buf.get(..n).unwrap_or_default()))
}

/// Read until `buf` is full or end of input, retrying interrupted reads.
/// Returns the number of bytes read.
pub(crate) fn read_full<R: Read + ?Sized>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while let Some(rest) = buf.get_mut(filled..) {
        if rest.is_empty() {
            break;
        }
        match reader.read(rest) {
            Ok(0) => break,
            // Bounded by `buf.len()`.
            Ok(n) => filled = filled.saturating_add(n.min(rest.len())),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn upstream_is_text_cases() {
        for (data, expected) in [
            (&b"hello world"[..], true),
            (b"", true),
            ("éóñəå  ⊂(◉‿◉)つ(ノ≥∇≤)ノ".as_bytes(), true),
            (r"¯\_(ツ)_/¯".as_bytes(), true),
            ("♪┏(・o･)┛♪┗ ( ･o･) ┓♪┏ ( ) ┛♪┗ (･o･ ) ┓♪".as_bytes(), true),
            (b"\xe9\xf3\xf1\xe5", true),
            (b"hello world\x00", false),
            (b"\x7f\x45\x4c\x46\x02\x01\x01", false),
            (b"\x43\x92\xd9\x0f\xaf\x32\x2c", false),
        ] {
            assert_eq!(is_text_bytes(data), expected, "{data:?}");
            assert_eq!(is_text(Cursor::new(data)).ok(), Some(expected), "{data:?}");
        }
    }

    #[test]
    fn only_first_1024_bytes_count() {
        let mut data = vec![b'a'; TEXT_PROBE_LEN];
        data.push(0);
        assert!(is_text_bytes(&data));
        assert!(is_text(Cursor::new(&data)).unwrap_or(false));
        data[TEXT_PROBE_LEN - 1] = 0;
        assert!(!is_text_bytes(&data));
    }

    #[test]
    fn short_reads_are_completed() {
        // identify (BufferedReader.read(1024)) keeps reading after a short read.
        let reader = Cursor::new(b"abc".to_vec()).chain(Cursor::new(b"\x00\x01".to_vec()));
        assert!(!is_text(reader).unwrap_or(true));
    }

    #[test]
    fn interrupted_reads_are_retried() {
        struct Flaky(u8);
        impl Read for Flaky {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 => Err(io::ErrorKind::Interrupted.into()),
                    2 => {
                        buf[0] = 0;
                        Ok(1)
                    }
                    _ => Ok(0),
                }
            }
        }
        assert!(!is_text(Flaky(0)).unwrap_or(true));
    }

    #[test]
    fn byte_classes() {
        for b in 0u8..=255 {
            let expected = matches!(b, 7 | 8 | 9 | 10 | 11 | 12 | 13 | 27)
                || (0x20..0x7f).contains(&b)
                || b >= 0x80;
            assert_eq!(is_text_bytes(&[b]), expected, "{b:#x}");
        }
    }
}
