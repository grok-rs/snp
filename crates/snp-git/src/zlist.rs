//! NUL-separated byte buffer + spans.
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-18")]

/// NUL-separated byte buffer + spans, git order preserved, never sorted (02 G.0 #8).
#[derive(Debug, Clone, Default)]
pub struct ZList {
    /// W0 placeholder; WP-18 stores buf: `Vec<u8>`, spans: `Vec<(u32, u32)>`.
    _priv: (),
}

impl ZList {
    /// Number of entries.
    pub fn len(&self) -> usize {
        todo!()
    }

    /// No entries.
    pub fn is_empty(&self) -> bool {
        todo!()
    }

    /// Entry `i`.
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        todo!()
    }

    /// Entries in git order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        core::iter::from_fn(|| todo!())
    }

    /// (buf, spans).
    pub fn into_parts(self) -> (Vec<u8>, Vec<(u32, u32)>) {
        todo!()
    }
}
