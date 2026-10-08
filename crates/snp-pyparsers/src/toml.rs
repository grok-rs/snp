//! `check-toml`: CPython 3.13 `tomllib` acceptance and messages (09 §4.C; `check_toml.py`).
//!
//! Owner: WP-17.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-17")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-17")]

/// `tomllib.loads(bytes.decode())`; `Err` is the `tomllib.TOMLDecodeError` text.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    todo!()
}
