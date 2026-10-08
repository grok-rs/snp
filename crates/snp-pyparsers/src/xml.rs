//! `check-xml`: expat well-formedness with expat's messages (09 §4.C; `check_xml.py`).
//!
//! Owner: WP-17.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-17")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-17")]

/// `xml.sax.parse(…)` (expat); `Err` is the `ExpatError` text.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    todo!()
}
