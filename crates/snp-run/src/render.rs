//! Pure renderer: events → bytes, snapshot-tested without processes (10 §9.1; `commands/run.py` `_full_msg`,
//! `_subtle_line`, `_run_single_hook` output).
//!
//! Every function here is pure (no I/O): the scheduler hands the results to the `Printer` in pre-commit's write
//! order (`output.write` for the start line, flushed before the hook runs; `output.write_line` for the status word
//! and each detail line).
//!
//! Owner: WP-32.

use snp_pycompat::eaw::len_cjk;
use snp_pycompat::float::format_duration;
use snp_pycompat::text::bytes_strip;

/// `color.RED` (`color.py:59`): `Failed`, `[ERROR]`.
const RED: &str = "\x1b[41m";
/// `color.GREEN`: `Passed`.
const GREEN: &str = "\x1b[42m";
/// `color.YELLOW`: `Skipped` (SKIP), `[WARNING]`.
const YELLOW: &str = "\x1b[43;30m";
/// `color.TURQUOISE`: `Skipped` (no files).
const TURQUOISE: &str = "\x1b[46;30m";
/// `color.SUBTLE`: the detail lines.
const SUBTLE: &str = "\x1b[2m";
/// `color.NORMAL`.
const NORMAL: &str = "\x1b[m";

/// `len('(no files to check)')`.
const NO_FILES: &str = "(no files to check)";

/// `max(80, max len_cjk + 30)` (`_compute_cols`).
///
/// `run.py:238-250`: `max(80, max_width + 3 + len('(no files to check)') + 1 + len('Skipped'))` over **every**
/// selected hook (skipped ones included), widths by Python's `unicodedata.east_asian_width` (W/F count 2).
pub fn cols(names: &[&str]) -> usize {
    let widest = names.iter().map(|n| len_cjk(n)).max().unwrap_or(0);
    // 3 dots + "(no files to check)" + 1 + "Skipped" = 30
    widest.saturating_add(30).max(80)
}

/// `{name}{dots}` before the hook runs (`_full_msg` start).
///
/// `_start_msg(start=name, end_len=6, cols=cols)` (`run.py:36-38`): `name + '.' * (cols - len_cjk(name) - 6 - 1)`;
/// a negative count is Python's empty string.
pub fn start_msg(name: &str, cols: usize) -> String {
    dotted(name, cols, 6)
}

/// The coloured status word and newline.
///
/// `Passed`/`Failed`/`Skipped` coloured with `format_color` (`{color}{word}\x1b[m` when colour is on). `NoFiles`
/// renders the uncoloured `(no files to check)` postfix followed by the turquoise `Skipped` — the postfix belongs to
/// the dotted part of `_full_msg`, so [`full_msg`] uses one dot less per postfix character.
pub fn end_msg(status: Status, color: bool) -> String {
    let (word, code) = match status {
        Status::Passed => ("Passed", GREEN),
        Status::Failed => ("Failed", RED),
        Status::Skipped => ("Skipped", YELLOW),
        Status::NoFiles => ("Skipped", TURQUOISE),
    };
    let mut s = String::new();
    if status == Status::NoFiles {
        s.push_str(NO_FILES);
    }
    push_colored(&mut s, word, code, color);
    s.push('\n');
    s
}

/// A complete status line for a hook that did not run (`Skipped` / `NoFiles`), or the line of a hook rendered after
/// it ran (parallel buckets, quiet output).
///
/// `_full_msg(start=name, postfix=…, end_msg=…, end_color=…, use_color=…, cols=cols)` (`run.py:41-57`):
/// `name + '.' * (cols - len_cjk(name) - len(postfix) - len(end_msg) - 1) + postfix + color(end_msg) + '\n'`.
/// For `Passed`/`Failed` the result equals `start_msg(name, cols) + end_msg(status, color)`.
pub fn full_msg(name: &str, status: Status, cols: usize, color: bool) -> String {
    let end_len = match status {
        Status::Passed | Status::Failed => 6,
        Status::Skipped => 7,
        Status::NoFiles => NO_FILES.len().saturating_add(7),
    };
    let mut s = dotted(name, cols, end_len);
    s.push_str(&end_msg(status, color));
    s
}

/// `name + '.' * (cols - len_cjk(name) - end_len - 1)`.
fn dotted(name: &str, cols: usize, end_len: usize) -> String {
    let dots = cols
        .saturating_sub(len_cjk(name))
        .saturating_sub(end_len)
        .saturating_sub(1);
    let mut s = String::from(name);
    s.extend(core::iter::repeat_n('.', dots));
    s
}

/// `format_color(text, color, use_color)` (`color.py:66-79`): even an empty colour appends the reset.
fn push_colored(out: &mut String, text: &str, code: &str, color: bool) {
    if color {
        out.push_str(code);
        out.push_str(text);
        out.push_str(NORMAL);
    } else {
        out.push_str(text);
    }
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
///
/// `run.py:217-233`: `_subtle_line(f'- hook id: {hook.id}')`, `- duration: {duration}s` (only when given: the hook
/// ran and is verbose; `round(dt, 2) or 0` printed with `repr(float)`), `- exit code: {retcode}` (non-zero only),
/// `- files were modified by this hook`, then snp's extension lines, then — when `out.strip()` (`bytes.strip`) is
/// non-empty — an empty line, the stripped output, `\n` and another empty line. Subtle lines are
/// `\x1b[2m{line}\x1b[m` with colour on.
pub fn details(d: &Details<'_>) -> Vec<u8> {
    let mut s = String::new();
    subtle(&mut s, &format!("- hook id: {}", d.id), d.color);
    if let Some(dur) = d.duration {
        subtle(
            &mut s,
            &format!("- duration: {}s", format_duration(dur)),
            d.color,
        );
    }
    if let Some(code) = d.exit_code.filter(|c| *c != 0) {
        subtle(&mut s, &format!("- exit code: {code}"), d.color);
    }
    if d.modified {
        subtle(&mut s, "- files were modified by this hook", d.color);
    }
    for line in d.ext_lines {
        subtle(&mut s, line, d.color);
    }
    let mut out = s.into_bytes();
    let stripped = bytes_strip(d.output);
    if !stripped.is_empty() {
        out.push(b'\n');
        out.extend_from_slice(stripped);
        out.extend_from_slice(b"\n\n");
    }
    out
}

/// `_subtle_line(s, use_color)`: `output.write_line(color.format_color(s, color.SUBTLE, use_color))`.
fn subtle(out: &mut String, line: &str, color: bool) {
    push_colored(out, line, SUBTLE, color);
    out.push('\n');
}

/// The payload pre-commit appends to `log_file` (`output.write_line_b(out.strip(), logfile_name=…)`): the stripped
/// output plus `\n`, or `None` when the output is blank (nothing is written then, `output.py:14-29`).
pub fn log_payload(output: &[u8]) -> Option<Vec<u8>> {
    let stripped = bytes_strip(output);
    if stripped.is_empty() {
        return None;
    }
    let mut v = stripped.to_vec();
    v.push(b'\n');
    Some(v)
}

/// One row of the `summary` output mode (snp extension, ARCHITECTURE §8.2).
#[derive(Debug, Clone)]
pub struct SummaryRow<'a> {
    /// The hook's name.
    pub name: &'a str,
    /// Its status.
    pub status: Status,
    /// Its wall time (hooks that ran).
    pub duration: Option<f64>,
}

/// The `output: summary` footer (snp extension): one line per hook that ran with its duration, then the counts and
/// the wall time.
///
/// ```text
/// ===============================================================================
/// Summary: 2 passed, 1 failed, 1 skipped in 0.52s
/// - fmt: 0.12s
/// - lint: 0.4s
/// ```
pub fn summary(rows: &[SummaryRow<'_>], wall: f64, color: bool) -> String {
    let count = |want: &[Status]| rows.iter().filter(|r| want.contains(&r.status)).count();
    let passed = count(&[Status::Passed]);
    let failed = count(&[Status::Failed]);
    let skipped = count(&[Status::Skipped, Status::NoFiles]);
    let mut s = String::new();
    s.extend(core::iter::repeat_n('=', 79));
    s.push('\n');
    let mut parts = Vec::new();
    let mut part = |n: usize, word: &str, code: &str| {
        let mut p = String::new();
        let text = format!("{n} {word}");
        if n == 0 {
            p.push_str(&text);
        } else {
            push_colored(&mut p, &text, code, color);
        }
        parts.push(p);
    };
    part(passed, "passed", GREEN);
    part(failed, "failed", RED);
    part(skipped, "skipped", YELLOW);
    s.push_str("Summary: ");
    s.push_str(&parts.join(", "));
    s.push_str(" in ");
    s.push_str(&format_duration(wall));
    s.push_str("s\n");
    for r in rows {
        if let Some(d) = r.duration {
            subtle(
                &mut s,
                &format!("- {}: {}s", r.name, format_duration(d)),
                color,
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t10_run_01_start_msg() {
        // _start_msg(start='start', end_len=5, cols=15) == 'start....'
        assert_eq!(dotted("start", 15, 5), "start....");
        assert_eq!(start_msg("start", 15), "start...");
    }

    #[test]
    fn t10_run_02_full_msg() {
        // _full_msg('start', 'end', cols=15): end_len == 3
        let full = |name: &str, end: &str, postfix: &str, cols: usize, color: Option<&str>| {
            let mut s = dotted(name, cols, end.len() + postfix.len());
            s.push_str(postfix);
            match color {
                Some(c) => push_colored(&mut s, end, c, true),
                None => s.push_str(end),
            }
            s.push('\n');
            s
        };
        assert_eq!(full("start", "end", "", 15, None), "start......end\n");
        assert_eq!(full("啊あ아", "end", "", 15, None), "啊あ아.....end\n");
        assert_eq!(
            full("start", "end", "", 15, Some(RED)),
            "start......\x1b[41mend\x1b[m\n"
        );
        assert_eq!(
            full("start", "end", "post ", 20, None),
            "start......post end\n"
        );
    }

    #[test]
    fn t10_run_03_compute_cols() {
        assert_eq!(cols(&[]), 80);
        let a51: String = core::iter::repeat_n('a', 51).collect();
        let a52: String = core::iter::repeat_n('a', 52).collect();
        assert_eq!(cols(&[&a51]), 81);
        assert_eq!(cols(&[&a51, &a52]), 82);
    }

    #[test]
    fn status_lines_are_cols_minus_one_wide() {
        for st in [
            Status::Passed,
            Status::Failed,
            Status::Skipped,
            Status::NoFiles,
        ] {
            let line = full_msg("passing hook", st, 80, false);
            assert_eq!(line.trim_end().chars().count(), 79, "{line:?}");
        }
        assert_eq!(
            full_msg("nothing matches", Status::NoFiles, 80, false),
            "nothing matches......................................(no files to check)Skipped\n"
        );
        assert_eq!(
            start_msg("passing hook", 80) + &end_msg(Status::Passed, false),
            "passing hook.............................................................Passed\n"
        );
    }

    #[test]
    fn colours_match_pre_commit() {
        assert_eq!(end_msg(Status::Passed, true), "\x1b[42mPassed\x1b[m\n");
        assert_eq!(end_msg(Status::Failed, true), "\x1b[41mFailed\x1b[m\n");
        assert_eq!(end_msg(Status::Skipped, true), "\x1b[43;30mSkipped\x1b[m\n");
        assert_eq!(
            end_msg(Status::NoFiles, true),
            "(no files to check)\x1b[46;30mSkipped\x1b[m\n"
        );
    }

    #[test]
    fn cjk_and_emoji_widths() {
        // 12 wide characters → 24 columns → 49 dots before `Passed` at cols 80
        let name = "检查中文名称の長いフック";
        let line = start_msg(name, 80);
        assert_eq!(line.matches('.').count(), 49);
        // the emoji counts 2, the combining mark 1 (Python's east_asian_width, not wcwidth)
        let name = "emoji 🎉 é combining e\u{301}";
        assert_eq!(len_cjk(name), name.chars().count() + 1);
    }

    #[test]
    fn details_block() {
        let ext = vec![String::from("- hook timed out after 1s")];
        let d = Details {
            id: "fail",
            duration: Some(0.034),
            exit_code: Some(3),
            modified: true,
            ext_lines: &ext,
            output: b"  out\nerr\n\n",
            color: false,
        };
        assert_eq!(
            String::from_utf8(details(&d)).unwrap(),
            "- hook id: fail\n- duration: 0.03s\n- exit code: 3\n- files were modified by this hook\n\
             - hook timed out after 1s\n\nout\nerr\n\n"
        );
        let d = Details {
            id: "x",
            duration: None,
            exit_code: Some(0),
            modified: false,
            ext_lines: &[],
            output: b" \t\n",
            color: true,
        };
        assert_eq!(
            String::from_utf8(details(&d)).unwrap(),
            "\x1b[2m- hook id: x\x1b[m\n"
        );
        assert_eq!(log_payload(b"\n a \n"), Some(b"a\n".to_vec()));
        assert_eq!(log_payload(b"\n"), None);
    }

    #[test]
    fn duration_formatting() {
        let block = |d: f64| {
            String::from_utf8(details(&Details {
                id: "identity",
                duration: Some(d),
                exit_code: None,
                modified: false,
                ext_lines: &[],
                output: b"",
                color: false,
            }))
            .unwrap()
        };
        // T10-RUN-15: 1.234 → 2.0 is 0.766 → 0.77; 1.0 → 1.0 is 0
        assert_eq!(
            block(2.0 - 1.234),
            "- hook id: identity\n- duration: 0.77s\n"
        );
        assert_eq!(block(0.0), "- hook id: identity\n- duration: 0s\n");
    }

    #[test]
    fn summary_footer() {
        let rows = [
            SummaryRow {
                name: "a",
                status: Status::Passed,
                duration: Some(0.12),
            },
            SummaryRow {
                name: "b",
                status: Status::NoFiles,
                duration: None,
            },
        ];
        let s = summary(&rows, 0.5, false);
        assert!(s.ends_with("Summary: 1 passed, 0 failed, 1 skipped in 0.5s\n- a: 0.12s\n"));
    }
}
