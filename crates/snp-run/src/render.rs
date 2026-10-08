//! Pure renderer: events → bytes, snapshot-tested without processes (10 §9.1; `commands/run.py` `_full_msg`,
//! `_subtle_line`, `_run_single_hook` output).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-32")]

/// `max(80, max len_cjk + 30)` (`_compute_cols`).
pub fn cols(names: &[&str]) -> usize {
    todo!()
}

/// `{name}{dots}` before the hook runs (`_full_msg` start).
pub fn start_msg(name: &str, cols: usize) -> String {
    todo!()
}

/// The coloured status word and newline.
pub fn end_msg(status: Status, color: bool) -> String {
    todo!()
}

/// A hook's result status.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// `Passed`.
    Passed,
    /// `Failed`.
    Failed,
    /// `Skipped`.
    Skipped,
    /// `(no files to check)Skipped`.
    NoFiles,
}

/// The block printed after a failed (or verbose) hook.
#[derive(Debug)]
pub struct Details<'a> {
    /// `- hook id: …`.
    pub id: &'a str,
    /// `- duration: …` (verbose).
    pub duration: Option<f64>,
    /// `- exit code: …`.
    pub exit_code: Option<i64>,
    /// `- files were modified by this hook`.
    pub modified: bool,
    /// Extension lines (`- hook timed out after …`, `- <fail_text>`, …).
    pub ext_lines: &'a [String],
    /// The hook's output.
    pub output: &'a [u8],
    /// The colour decision.
    pub color: bool,
}

/// Renders a details block.
pub fn details(d: &Details<'_>) -> Vec<u8> {
    todo!()
}
