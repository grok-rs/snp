//! The production finder (trusted, differential-tested against `kern::Scalar`).

/// memchr-backed finder.
pub(crate) struct Fast;

impl crate::kern::FindByte for Fast {
    fn find_byte(&self, needle: u8, hay: &[u8], from: usize) -> Option<usize> {
        let tail = hay.get(from..)?;
        tail.iter().position(|b| *b == needle).map(|i| i.saturating_add(from))
    }
}
