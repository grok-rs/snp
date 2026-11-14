//! Helpers shared by the fuzz targets.

use std::io::{self, Read};

/// A reader over `data` that returns at most `chunk` bytes per call and,
/// when `interrupt` is set, fails every other call with
/// [`io::ErrorKind::Interrupted`] (which readers must retry).
pub struct Chunked<'a> {
    data: &'a [u8],
    chunk: usize,
    interrupt: bool,
    calls: usize,
}

impl<'a> Chunked<'a> {
    /// `control` picks the chunk size (1..=128) and whether to interrupt.
    pub fn new(data: &'a [u8], control: u8) -> Self {
        Self {
            data,
            chunk: usize::from(control & 0x7f) + 1,
            interrupt: control & 0x80 != 0,
            calls: 0,
        }
    }
}

impl Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.interrupt && self.calls % 2 == 1 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let n = buf.len().min(self.chunk).min(self.data.len());
        let (now, later) = self.data.split_at(n);
        buf[..n].copy_from_slice(now);
        self.data = later;
        Ok(n)
    }
}

/// Split `data` into a control byte and the rest.
pub fn control(data: &[u8]) -> (u8, &[u8]) {
    match data.split_first() {
        Some((&c, rest)) => (c, rest),
        None => (0, data),
    }
}
