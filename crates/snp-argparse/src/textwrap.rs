//! CPython `textwrap` as used by `argparse.HelpFormatter` (`_split_lines`, `_fill_text`).
//!
//! Owner: WP-12.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-12")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-12")]

/// `textwrap.wrap(text, width)`.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    todo!()
}

/// `textwrap.fill(text, width, initial_indent=…, subsequent_indent=…)`.
pub fn fill(text: &str, width: usize, initial_indent: &str, subsequent_indent: &str) -> String {
    todo!()
}
