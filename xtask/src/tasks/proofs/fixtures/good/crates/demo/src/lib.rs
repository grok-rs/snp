//! Demo crate of the guard fixtures: the shell that calls the kernel.
mod fast;
mod kern;

/// Splits `hay` at newlines (unverified shell of kernel K9).
pub fn split(hay: &[u8]) -> Vec<std::ops::Range<usize>> {
    kern::spans(&fast::Fast, b'\n', hay)
}
