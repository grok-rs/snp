//! The `configparser` subset `detect-aws-credentials` uses (09 §4.D; `detect_aws_credentials.py`).
//!
//! Owner: WP-17.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-17")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-17")]

/// `configparser.RawConfigParser().read_string(text)`: every `(option, value)` pair in file order; `Err` is the
/// `configparser.Error` text.
pub fn aws_credentials(text: &str) -> Result<Vec<(String, String)>, String> {
    todo!()
}
