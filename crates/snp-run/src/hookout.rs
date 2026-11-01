//! A hook's combined output: the partitions' merged outputs in partition order, each in memory or spilled
//! (ADR-036), with `bytes.strip()` applied across the concatenation without materialising a spilled part.
//!
//! Owner: WP-32.

use std::io::{self, Write};

use snp_proc::spawn::Output;

/// `bytes.strip()` whitespace: space, `\t`, `\n`, `\r`, `\x0b`, `\x0c`.
const fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `usize` → `u64` (lossless on every supported target).
fn u64_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// The concatenated output of a hook.
#[derive(Debug, Default)]
pub(crate) struct HookOutput {
    parts: Vec<Output>,
}

impl HookOutput {
    /// An in-memory output.
    pub(crate) fn bytes(b: Vec<u8>) -> Self {
        Self {
            parts: vec![Output::Memory(b)],
        }
    }

    /// Appends one partition's output.
    pub(crate) fn push(&mut self, o: Output) {
        self.parts.push(o);
    }

    /// Appends in-memory bytes.
    pub(crate) fn push_bytes(&mut self, b: Vec<u8>) {
        if !b.is_empty() {
            self.parts.push(Output::Memory(b));
        }
    }

    /// `not out.strip()`.
    pub(crate) fn is_blank(&self) -> bool {
        self.parts.iter().all(Output::is_blank)
    }

    /// The whole output (tests, small outputs).
    #[cfg(test)]
    pub(crate) fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::new();
        for p in &self.parts {
            p.write_to(&mut v).unwrap();
        }
        v
    }

    /// Writes `out.strip()` into `w` in chunks: pass 1 finds the first and the last non-whitespace byte, pass 2
    /// copies the range (a spilled part is read twice, never held in memory).
    pub(crate) fn write_stripped(&self, w: &mut dyn Write) -> io::Result<()> {
        let mut scan = Scan::default();
        for p in &self.parts {
            p.write_to(&mut scan)?;
        }
        let (Some(start), Some(end)) = (scan.first, scan.last_end) else {
            return Ok(());
        };
        let mut range = Range {
            pos: 0,
            start,
            end,
            inner: w,
        };
        for p in &self.parts {
            p.write_to(&mut range)?;
        }
        Ok(())
    }
}

/// Pass 1: offsets of the first non-whitespace byte and one past the last.
#[derive(Debug, Default)]
struct Scan {
    pos: u64,
    first: Option<u64>,
    last_end: Option<u64>,
}

impl Write for Scan {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(i) = buf.iter().position(|b| !is_ws(*b)) {
            if self.first.is_none() {
                self.first = Some(self.pos.saturating_add(u64_of(i)));
            }
            if let Some(j) = buf.iter().rposition(|b| !is_ws(*b)) {
                self.last_end = Some(self.pos.saturating_add(u64_of(j)).saturating_add(1));
            }
        }
        self.pos = self.pos.saturating_add(u64_of(buf.len()));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Pass 2: forwards the bytes in `start..end`.
struct Range<'a> {
    pos: u64,
    start: u64,
    end: u64,
    inner: &'a mut dyn Write,
}

impl Write for Range<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = u64_of(buf.len());
        let lo = self.start.saturating_sub(self.pos).min(len);
        let hi = self.end.saturating_sub(self.pos).min(len);
        if lo < hi {
            let (lo, hi) = (
                usize::try_from(lo).unwrap_or(0),
                usize::try_from(hi).unwrap_or(0),
            );
            if let Some(s) = buf.get(lo..hi) {
                self.inner.write_all(s)?;
            }
        }
        self.pos = self.pos.saturating_add(len);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A `Write` that forwards each chunk to the printer's byte layer (`sys.stdout.buffer.write` + flush).
pub(crate) struct PrinterSink<'a>(pub(crate) &'a snp_core::output::Printer);

impl Write for PrinterSink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripped(parts: &[&[u8]]) -> Vec<u8> {
        let mut o = HookOutput::default();
        for p in parts {
            o.push_bytes(p.to_vec());
        }
        let mut v = Vec::new();
        o.write_stripped(&mut v).unwrap();
        v
    }

    #[test]
    fn strip_across_partitions() {
        assert_eq!(stripped(&[b"  a\n", b"b  \n\n"]), b"a\nb");
        assert_eq!(stripped(&[b" \n", b"\t"]), b"");
        assert_eq!(stripped(&[b"", b"x", b" "]), b"x");
        assert_eq!(stripped(&[b"\x0b\x0cx y\r\n"]), b"x y");
        assert_eq!(
            stripped(&[b"  out-with-spaces  \nerr\n"]),
            b"out-with-spaces  \nerr"
        );
        let mut o = HookOutput::bytes(b" \n".to_vec());
        assert!(o.is_blank());
        o.push_bytes(b"z".to_vec());
        assert!(!o.is_blank());
        assert_eq!(o.to_vec(), b" \nz");
    }
}
