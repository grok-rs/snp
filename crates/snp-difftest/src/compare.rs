//! Comparison of normalised streams and states, and the unified-diff rendering of a difference.
//!
//! Owner: WP-35.

use crate::scenario::{ExitMode, StreamMode};
use crate::text::{lines_with_ends, show};

/// One observed difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// What differs: `step 2 (run) stdout`, `step 1 tree`, …
    pub what: String,
    /// A unified diff (oracle = `-`, snp = `+`).
    pub diff: String,
}

/// Whether two normalised streams agree under `mode`.
pub(crate) fn streams_equal(mode: StreamMode, a: &[u8], b: &[u8]) -> bool {
    match mode {
        StreamMode::Ignore => true,
        StreamMode::LinesUnordered => {
            let mut x = lines_with_ends(a);
            let mut y = lines_with_ends(b);
            x.sort_unstable();
            y.sort_unstable();
            x == y
        }
        StreamMode::Exact | StreamMode::Raw | StreamMode::TierB => a == b,
    }
}

/// Whether two exit codes agree under `mode` (`None` = killed at the deadline).
pub(crate) fn exits_equal(mode: ExitMode, a: Option<i32>, b: Option<i32>) -> bool {
    match mode {
        ExitMode::Ignore => true,
        ExitMode::Exact => a == b,
        ExitMode::NonzeroEqual => a.map(|c| c == 0) == b.map(|c| c == 0),
    }
}

/// Upper bound on the LCS table (lines × lines) before the diff falls back to a head/tail rendering.
const MAX_CELLS: usize = 4_000_000;

/// A unified diff of `a` (oracle) and `b` (snp) with 3 lines of context. Non-printable bytes are shown escaped
/// so that the report is readable (`\x1b[41m`, `\r`).
pub(crate) fn unified(a: &[u8], b: &[u8]) -> String {
    let x = lines_with_ends(a);
    let y = lines_with_ends(b);
    let cells = x
        .len()
        .saturating_add(1)
        .saturating_mul(y.len().saturating_add(1));
    let ops = if cells <= MAX_CELLS {
        lcs_ops(&x, &y)
    } else {
        fallback_ops(&x, &y)
    };
    render(&ops)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op<'a> {
    Keep(&'a [u8]),
    Del(&'a [u8]),
    Add(&'a [u8]),
}

#[expect(
    clippy::many_single_char_names,
    reason = "the textbook LCS notation (x, y, n, m, i, j)"
)]
fn lcs_ops<'a>(x: &[&'a [u8]], y: &[&'a [u8]]) -> Vec<Op<'a>> {
    let n = x.len();
    let m = y.len();
    let w = m.saturating_add(1);
    // table[i][j] = LCS length of x[i..] and y[j..]
    let mut table = vec![0_u32; n.saturating_add(1).saturating_mul(w)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let here = i.saturating_mul(w).saturating_add(j);
            let v = if x.get(i) == y.get(j) {
                table
                    .get(
                        (i.saturating_add(1))
                            .saturating_mul(w)
                            .saturating_add(j.saturating_add(1)),
                    )
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1)
            } else {
                let down = table
                    .get(i.saturating_add(1).saturating_mul(w).saturating_add(j))
                    .copied()
                    .unwrap_or(0);
                let right = table.get(here.saturating_add(1)).copied().unwrap_or(0);
                down.max(right)
            };
            if let Some(c) = table.get_mut(here) {
                *c = v;
            }
        }
    }
    let at = |i: usize, j: usize| {
        table
            .get(i.saturating_mul(w).saturating_add(j))
            .copied()
            .unwrap_or(0)
    };
    let mut ops = Vec::new();
    let (mut i, mut j) = (0_usize, 0_usize);
    while i < n || j < m {
        match (x.get(i), y.get(j)) {
            (Some(a), Some(b)) if a == b => {
                ops.push(Op::Keep(a));
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            (Some(a), Some(_)) if at(i.saturating_add(1), j) >= at(i, j.saturating_add(1)) => {
                ops.push(Op::Del(a));
                i = i.saturating_add(1);
            }
            (_, Some(b)) => {
                ops.push(Op::Add(b));
                j = j.saturating_add(1);
            }
            (Some(a), None) => {
                ops.push(Op::Del(a));
                i = i.saturating_add(1);
            }
            (None, None) => break,
        }
    }
    ops
}

/// For huge inputs: common prefix and suffix kept, the middle shown as one deletion and one addition.
fn fallback_ops<'a>(x: &[&'a [u8]], y: &[&'a [u8]]) -> Vec<Op<'a>> {
    let pre = x.iter().zip(y).take_while(|(a, b)| a == b).count();
    let suf = x
        .iter()
        .rev()
        .zip(y.iter().rev())
        .take(x.len().min(y.len()).saturating_sub(pre))
        .take_while(|(a, b)| a == b)
        .count();
    let mut ops: Vec<Op<'a>> = x.iter().take(pre).map(|l| Op::Keep(l)).collect();
    ops.extend(
        x.iter()
            .skip(pre)
            .take(x.len().saturating_sub(pre).saturating_sub(suf))
            .map(|l| Op::Del(l)),
    );
    ops.extend(
        y.iter()
            .skip(pre)
            .take(y.len().saturating_sub(pre).saturating_sub(suf))
            .map(|l| Op::Add(l)),
    );
    ops.extend(
        x.iter()
            .skip(x.len().saturating_sub(suf))
            .map(|l| Op::Keep(l)),
    );
    ops
}

fn escape_line(l: &[u8]) -> String {
    let body = l.strip_suffix(b"\n").unwrap_or(l);
    let s = show(body);
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\t' => out.push('\t'),
            c if c.is_control() => out.push_str(&c.escape_default().to_string()),
            c => out.push(c),
        }
    }
    if !l.ends_with(b"\n") {
        out.push_str("\\ (no newline)");
    }
    out
}

const CONTEXT: usize = 3;

fn render(ops: &[Op<'_>]) -> String {
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| !matches!(o, Op::Keep(_)))
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut shown_until = 0_usize;
    let mut i = 0_usize;
    while let Some(&first) = changed.get(i) {
        let start = first.saturating_sub(CONTEXT).max(shown_until);
        // extend the hunk while the next change is within 2*CONTEXT
        let mut last = first;
        let mut k = i.saturating_add(1);
        while let Some(&c) = changed.get(k) {
            if c.saturating_sub(last) <= CONTEXT.saturating_mul(2) {
                last = c;
                k = k.saturating_add(1);
            } else {
                break;
            }
        }
        let end = last
            .saturating_add(CONTEXT)
            .saturating_add(1)
            .min(ops.len());
        if start > shown_until || start == 0 {
            out.push_str("@@\n");
        }
        for op in ops.get(start..end).unwrap_or_default() {
            match op {
                Op::Keep(l) => {
                    out.push(' ');
                    out.push_str(&escape_line(l));
                }
                Op::Del(l) => {
                    out.push('-');
                    out.push_str(&escape_line(l));
                }
                Op::Add(l) => {
                    out.push('+');
                    out.push_str(&escape_line(l));
                }
            }
            out.push('\n');
        }
        shown_until = end;
        i = k;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert!(streams_equal(StreamMode::Exact, b"a\n", b"a\n"));
        assert!(!streams_equal(StreamMode::Exact, b"a\n", b"a"));
        assert!(streams_equal(
            StreamMode::LinesUnordered,
            b"a\nb\n",
            b"b\na\n"
        ));
        assert!(!streams_equal(
            StreamMode::LinesUnordered,
            b"a\nb\n",
            b"b\nb\n"
        ));
        assert!(streams_equal(StreamMode::Ignore, b"x", b"y"));
        assert!(exits_equal(ExitMode::NonzeroEqual, Some(1), Some(3)));
        assert!(!exits_equal(ExitMode::NonzeroEqual, Some(0), Some(3)));
        assert!(!exits_equal(ExitMode::Exact, Some(1), None));
        assert!(exits_equal(ExitMode::Ignore, Some(1), None));
    }

    #[test]
    fn unified_diff() {
        let d = unified(b"a\nb\nc\nd\n", b"a\nB\nc\nd\ne");
        assert_eq!(d, "@@\n a\n-b\n+B\n c\n d\n+e\\ (no newline)\n");
        assert_eq!(unified(b"same\n", b"same\n"), "");
        let d = unified(b"\x1b[41mFailed\x1b[m\r\n", b"Failed\n");
        assert!(d.contains("\\u{1b}[41mFailed\\u{1b}[m\\r"), "{d}");
        let a: Vec<u8> = (0..30)
            .flat_map(|i| format!("{i}\n").into_bytes())
            .collect();
        let mut b = a.clone();
        b.extend_from_slice(b"x\n");
        let d = unified(&a, &b);
        assert!(d.starts_with("@@\n 27\n 28\n 29\n+x\n"), "{d}");
    }

    #[test]
    fn fallback_for_large_inputs() {
        let x: Vec<&[u8]> = vec![b"a\n", b"b\n", b"c\n"];
        let y: Vec<&[u8]> = vec![b"a\n", b"X\n", b"c\n"];
        let ops = fallback_ops(&x, &y);
        assert_eq!(
            ops,
            vec![
                Op::Keep(b"a\n"),
                Op::Del(b"b\n"),
                Op::Add(b"X\n"),
                Op::Keep(b"c\n")
            ]
        );
    }
}
