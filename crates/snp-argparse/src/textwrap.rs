//! CPython 3.13 `textwrap` (`Lib/textwrap.py`) with the default `TextWrapper` settings argparse uses
//! (`HelpFormatter._split_lines` → [`wrap`], `HelpFormatter._fill_text` → [`fill`]): `expand_tabs=True`
//! (tab size 8), `replace_whitespace=True`, `break_long_words=True`, `break_on_hyphens=True`,
//! `drop_whitespace=True`, no `max_lines`.
//!
//! Lengths are Python `len()` (code points). The chunking reproduces `TextWrapper.wordsep_re` exactly: whitespace
//! runs, em-dashes between words, and words split after a hyphen that follows two letters (or letter-hyphen-letter)
//! and precedes letter(-)letter. `\w` uses Unicode letters and numbers (`char::is_alphanumeric`, `_`); for
//! non-ASCII text CPython's `str.isalnum()` differs from it only for a few combining marks.
//!
//! Owner: WP-12.

use std::ops::Range;

use snp_pycompat::chars::{decimal_value, is_space};

/// `textwrap._whitespace` (`'\t\n\x0b\x0c\r '`): the only characters `TextWrapper` treats as breakable spaces.
const fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

/// `\w` (Unicode).
fn is_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// `[^\d\W]`: a word character that is not a decimal digit (`_` included).
fn is_letter(c: char) -> bool {
    is_word(c) && decimal_value(c).is_none()
}

/// `[\w!"'&.,?]` (`word_punct`).
fn is_word_punct(c: char) -> bool {
    is_word(c) || matches!(c, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

/// `_munge_whitespace`: `str.expandtabs(8)` (the column restarts after `\n` and `\r`), then every `_whitespace`
/// character becomes a space.
fn munge(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut col = 0usize;
    for c in text.chars() {
        match c {
            '\t' => {
                let n = 8usize.saturating_sub(col % 8);
                out.extend(std::iter::repeat_n(' ', n));
                col = col.saturating_add(n);
            }
            '\n' | '\r' => {
                out.push(' ');
                col = 0;
            }
            c if is_ws(c) => {
                out.push(' ');
                col = col.saturating_add(1);
            }
            c => {
                out.push(c);
                col = col.saturating_add(1);
            }
        }
    }
    out
}

/// End of the `wordsep_re` match that starts at `start` (`start < text.len()`); every position starts a match.
fn chunk_end(text: &[char], start: usize) -> usize {
    let at = |i: usize| text.get(i).copied();
    let len = text.len();
    let Some(first) = at(start) else { return len };
    // `[ws]+`
    if is_ws(first) {
        let mut end = start;
        while at(end).is_some_and(is_ws) {
            end = end.saturating_add(1);
        }
        return end;
    }
    // `(?<=wp) -{2,} (?=\w)`: the whole hyphen run, followed by a word character.
    if first == '-' && start > 0 && at(start.saturating_sub(1)).is_some_and(is_word_punct) {
        let mut end = start;
        while at(end) == Some('-') {
            end = end.saturating_add(1);
        }
        if end.saturating_sub(start) >= 2 && at(end).is_some_and(is_word) {
            return end;
        }
    }
    // `[^ws]+? (?: hyphenated | end of word | before an em-dash )`, lazily: the shortest prefix that ends one way.
    let mut cut = start.saturating_add(1);
    loop {
        // `-(?: (?<=lt{2}-) | (?<=lt-lt-)) (?= lt -? lt)`
        if at(cut) == Some('-') {
            let two_letters = cut >= 2
                && at(cut.saturating_sub(2)).is_some_and(is_letter)
                && at(cut.saturating_sub(1)).is_some_and(is_letter);
            let letter_hyphen_letter = cut >= 3
                && at(cut.saturating_sub(3)).is_some_and(is_letter)
                && at(cut.saturating_sub(2)) == Some('-')
                && at(cut.saturating_sub(1)).is_some_and(is_letter);
            let ahead = at(cut.saturating_add(1)).is_some_and(is_letter)
                && (at(cut.saturating_add(2)).is_some_and(is_letter)
                    || (at(cut.saturating_add(2)) == Some('-')
                        && at(cut.saturating_add(3)).is_some_and(is_letter)));
            if (two_letters || letter_hyphen_letter) && ahead {
                return cut.saturating_add(1);
            }
        }
        // `(?=ws|\Z)`
        match at(cut) {
            None => return len,
            Some(c) if is_ws(c) => return cut,
            Some(_) => {}
        }
        // `(?<=wp) (?=-{2,}\w)`
        if at(cut.saturating_sub(1)).is_some_and(is_word_punct)
            && at(cut) == Some('-')
            && at(cut.saturating_add(1)) == Some('-')
        {
            let mut end = cut;
            while at(end) == Some('-') {
                end = end.saturating_add(1);
            }
            if at(end).is_some_and(is_word) {
                return cut;
            }
        }
        cut = cut.saturating_add(1);
    }
}

/// `TextWrapper._split`: the chunks of the munged text as ranges.
fn split(t: &[char]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < t.len() {
        let e = chunk_end(t, p).max(p.saturating_add(1));
        out.push(p..e);
        p = e;
    }
    out
}

/// `chunk.strip() == ''` (Python's Unicode whitespace).
fn blank(t: &[char], r: &Range<usize>) -> bool {
    t.get(r.clone())
        .unwrap_or_default()
        .iter()
        .all(|&c| is_space(c))
}

fn ilen(r: &Range<usize>) -> i64 {
    i64::try_from(r.len()).unwrap_or(i64::MAX)
}

/// `TextWrapper._wrap_chunks` with `width` and the two indents.
fn wrap_chunks(
    t: &[char],
    mut chunks: Vec<Range<usize>>,
    width: i64,
    initial_indent: &str,
    subsequent_indent: &str,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    chunks.reverse();
    while !chunks.is_empty() {
        let before = chunks.len();
        let before_last = chunks.last().cloned();
        let mut cur_line: Vec<Range<usize>> = Vec::new();
        let mut cur_len: i64 = 0;
        let indent = if lines.is_empty() {
            initial_indent
        } else {
            subsequent_indent
        };
        let line_width = width.saturating_sub(i64::try_from(indent.chars().count()).unwrap_or(0));
        if !lines.is_empty() && chunks.last().is_some_and(|c| blank(t, c)) {
            chunks.pop();
        }
        while let Some(c) = chunks.last() {
            let l = ilen(c);
            if cur_len.saturating_add(l) <= line_width {
                cur_len = cur_len.saturating_add(l);
                cur_line.extend(chunks.pop());
            } else {
                break;
            }
        }
        if chunks.last().is_some_and(|c| ilen(c) > line_width) {
            handle_long_word(t, &mut chunks, &mut cur_line, cur_len, line_width);
        }
        if cur_line.last().is_some_and(|c| blank(t, c)) {
            cur_line.pop();
        }
        if !cur_line.is_empty() {
            let mut line = String::from(indent);
            for r in &cur_line {
                line.extend(t.get(r.clone()).unwrap_or_default());
            }
            lines.push(line);
        } else if chunks.len() == before && chunks.last() == before_last.as_ref() {
            // No progress is impossible with `break_long_words=True`; guard against a loop all the same.
            chunks.pop();
        }
    }
    lines
}

/// `TextWrapper._handle_long_word` with `break_long_words=True`, `break_on_hyphens=True`.
fn handle_long_word(
    t: &[char],
    chunks: &mut [Range<usize>],
    cur_line: &mut Vec<Range<usize>>,
    cur_len: i64,
    width: i64,
) {
    let space_left = if width < 1 {
        1
    } else {
        width.saturating_sub(cur_len)
    };
    if space_left <= 0 {
        return;
    }
    let Some(last) = chunks.last_mut() else {
        return;
    };
    let chunk = last.clone();
    let space = usize::try_from(space_left).unwrap_or(usize::MAX);
    let mut end = space;
    if chunk.len() > space {
        // `chunk.rfind('-', 0, space_left)`, used only with non-hyphens before it.
        let head = t
            .get(chunk.start..chunk.start.saturating_add(space))
            .unwrap_or_default();
        if let Some(h) = head.iter().rposition(|&c| c == '-')
            && h > 0
            && head.get(..h).unwrap_or_default().iter().any(|&c| c != '-')
        {
            end = h.saturating_add(1);
        }
    }
    let split = chunk.start.saturating_add(end).min(chunk.end);
    cur_line.push(chunk.start..split);
    *last = split..chunk.end;
}

/// `textwrap.wrap(text, width)`: the wrapped lines without final newlines. CPython raises `ValueError` for
/// `width <= 0`; a `width` of 0 is treated as 1 here.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    wrap_indented(text, width, "", "")
}

/// `textwrap.wrap(text, width, initial_indent=…, subsequent_indent=…)`.
pub(crate) fn wrap_indented(
    text: &str,
    width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
) -> Vec<String> {
    let t = munge(text);
    let chunks = split(&t);
    let width = i64::try_from(width.max(1)).unwrap_or(i64::MAX);
    wrap_chunks(&t, chunks, width, initial_indent, subsequent_indent)
}

/// `textwrap.fill(text, width, initial_indent=…, subsequent_indent=…)`: the wrapped lines joined by `\n`.
pub fn fill(text: &str, width: usize, initial_indent: &str, subsequent_indent: &str) -> String {
    wrap_indented(text, width, initial_indent, subsequent_indent).join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(s: &str) -> Vec<String> {
        let t = munge(s);
        split(&t)
            .into_iter()
            .map(|r| t[r].iter().collect())
            .collect()
    }

    #[test]
    fn split_matches_wordsep_re() {
        // The examples of `TextWrapper._split`'s docstring.
        assert_eq!(
            chunks("Look, goof-ball -- use the -b option!"),
            [
                "Look,", " ", "goof-", "ball", " ", "--", " ", "use", " ", "the", " ", "-b", " ",
                "option!"
            ]
        );
        assert_eq!(
            chunks("Hello there -- you goof-ball, use the -b option!"),
            [
                "Hello", " ", "there", " ", "--", " ", "you", " ", "goof-", "ball,", " ", "use",
                " ", "the", " ", "-b", " ", "option!"
            ]
        );
        assert_eq!(chunks("Auto-update"), ["Auto-", "update"]);
        assert_eq!(chunks("pre-commit"), ["pre-", "commit"]);
        assert_eq!(chunks("--install-hooks"), ["--install-", "hooks"]);
        assert_eq!(chunks("a-b-c"), ["a-b-c"]);
        assert_eq!(chunks("ab-c"), ["ab-c"]);
        assert_eq!(chunks("ab-cd"), ["ab-", "cd"]);
        assert_eq!(chunks("x-ab-cd"), ["x-ab-", "cd"]);
        assert_eq!(chunks("a-b-cd"), ["a-b-", "cd"]);
        assert_eq!(chunks("word--word"), ["word", "--", "word"]);
        assert_eq!(chunks("\ta\tb"), ["        ", "a", "       ", "b"]);
    }

    #[test]
    fn wrap_like_python() {
        assert_eq!(
            wrap("show this help message and exit", 14),
            ["show this help", "message and", "exit"]
        );
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("aaa-bbbbbbbb", 6), ["aaa-bb", "bbbbbb"]);
        assert_eq!(wrap("   leading", 20), ["   leading"]);
        assert_eq!(wrap("a  b", 1), ["a", "b"]);
        assert_eq!(wrap("", 10), Vec::<String>::new());
        assert_eq!(wrap("   ", 10), Vec::<String>::new());
        assert_eq!(
            fill("one two three", 7, "> ", ". "),
            "> one\n. two\n. three"
        );
        assert_eq!(fill("x", 1, "    ", "    "), "    x");
    }
}
