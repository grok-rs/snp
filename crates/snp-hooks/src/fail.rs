//! Native `fail` language (`languages/fail.py`; §7.1).
//!
//! `run_hook` (`pre_commit/languages/fail.py:15-27`) ignores `args`, `require_serial`, `color` and the prefix and
//! returns `(1, f'{entry}\n\n'.encode() + b'\n'.join(f.encode() for f in file_args) + b'\n')`; pre-commit runs it
//! once with all files, unshuffled (04a §8.3). The two pre-commit-hooks `language: fail` hooks
//! (`check-illegal-windows-names`, `forbid-submodules`) need nothing else: their manifest entry is the message.
//!
//! Owner: WP-26.

/// `f'{entry}\n\n' + '\n'.join(files) + '\n'`.
///
/// The exit code is always 1 (the caller's). File names are the raw bytes of the classifier's repo-relative
/// paths (pre-commit encodes its `str` file names as UTF-8).
pub fn output(entry: &str, files: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(entry.as_bytes());
    out.extend_from_slice(b"\n\n");
    for (i, f) in files.iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(f);
    }
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::output;

    #[test]
    fn observed_outputs() {
        // 04a §8.3 (observed): two-line entry, four files.
        assert_eq!(
            output(
                "Files must not be txt\nsecond line",
                &[b"a.txt", b"b.txt", b"c.txt", b"d.txt"]
            ),
            b"Files must not be txt\nsecond line\n\na.txt\nb.txt\nc.txt\nd.txt\n"
        );
        // No files: the entry, a blank line, then the lone `\n` of the empty join.
        assert_eq!(output("nope", &[]), b"nope\n\n\n");
        // 09 §4.F: check-illegal-windows-names / forbid-submodules.
        assert_eq!(
            output("submodules are not allowed in this repository:", &[b"sub"]),
            b"submodules are not allowed in this repository:\n\nsub\n"
        );
        assert_eq!(
            output("x", &[b"caf\xc3\xa9", b"\xff"]),
            b"x\n\ncaf\xc3\xa9\n\xff\n"
        );
    }
}
