//! `pyvenv.cfg` reading and the `version_info` comparison of the health check (`python.py:36-46`, `:175-211`;
//! 04a §2.5, §14.9).
//!
//! Owner: WP-24.

use std::collections::HashMap;
use std::path::Path;

/// Cap on a `pyvenv.cfg` (a real one is ~300 bytes).
const MAX_PYVENV_CFG: u64 = 1 << 20;

/// `_read_pyvenv_cfg(filename)` (`python.py:36-46`) over the file's text: for each line (universal newlines, as
/// Python's text mode reads them), `k, v = line.split('=')` — a line that does not split into exactly two parts
/// (blank lines, comments, values containing `=`) is skipped — and `ret[k.strip()] = v.strip()` (a later key
/// overrides an earlier one, keeping the first position, like a `dict`).
pub(crate) fn parse(text: &str) -> Vec<(String, String)> {
    let text = snp_pycompat::textio::universal_newlines(text);
    let mut ret: Vec<(String, String)> = Vec::new();
    // key → position in `ret`; only for lookups (the vector keeps the order), so the parse is linear.
    let mut index: HashMap<String, usize> = HashMap::new();
    for line in text.split_inclusive('\n') {
        let mut parts = line.split('=');
        let (Some(k), Some(v), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let k = snp_pycompat::text::strip(k).to_owned();
        let v = snp_pycompat::text::strip(v).to_owned();
        if let Some(slot) = index.get(&k).and_then(|&i| ret.get_mut(i)) {
            slot.1 = v;
        } else {
            index.insert(k.clone(), ret.len());
            ret.push((k, v));
        }
    }
    ret
}

/// Reads and parses `path`; `None` when it does not exist or cannot be read. Non-UTF-8 bytes are decoded lossily
/// (pre-commit opens the file with `encoding='UTF-8'` and crashes with `UnicodeDecodeError`; D-CRASH-13).
pub(crate) fn read(path: &Path) -> Option<Vec<(String, String)>> {
    let bytes = snp_os::fs::read_capped(path, MAX_PYVENV_CFG).ok()?;
    Some(parse(&String::from_utf8_lossy(&bytes)))
}

/// The value of `key`.
pub(crate) fn get<'a>(cfg: &'a [(String, String)], key: &str) -> Option<&'a str> {
    cfg.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// The interpreter version recorded by the environment's creator: virtualenv and uv write `version_info`
/// (`3.12.3.final.0` and `3.12.3`), the standard library's `venv` (snp's pip backend) writes `version = 3.12.3`.
pub(crate) fn recorded_version(cfg: &[(String, String)]) -> Option<&str> {
    get(cfg, "version_info").or_else(|| get(cfg, "version"))
}

/// Whether the interpreter's `version_info` (`3.12.3.final.0`) matches the recorded one.
///
/// pre-commit compares the strings exactly, so it rejects every `uv venv` environment (uv writes `3.12.3`, G5);
/// snp also accepts the shorter `X.Y.Z` form when the shared `X.Y.Z` prefix matches (04a §14.9): two full forms must
/// still be equal, release level and serial included.
pub(crate) fn versions_match(actual: &str, recorded: &str) -> bool {
    if actual == recorded {
        return true;
    }
    let a: Vec<&str> = actual.split('.').collect();
    let r: Vec<&str> = recorded.split('.').collect();
    let (Some(a3), Some(r3)) = (a.get(..3), r.get(..3)) else {
        return false;
    };
    a3 == r3 && (a.len() == 3 || r.len() == 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `python_test.py:21-37` (`test_read_pyvenv_cfg`, `test_read_pyvenv_cfg_non_utf8`).
    #[test]
    fn lang_python_read_pyvenv_cfg() {
        let cfg = parse("# I am a comment\n\nfoo = bar\nversion-info=123\n");
        assert_eq!(
            cfg,
            vec![
                ("foo".to_owned(), "bar".to_owned()),
                ("version-info".to_owned(), "123".to_owned())
            ]
        );
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("pyvenv.cfg");
        std::fs::write(&p, "hello = hello john.\u{161}\n".as_bytes()).unwrap();
        assert_eq!(
            read(&p).unwrap(),
            vec![("hello".to_owned(), "hello john.\u{161}".to_owned())]
        );
        assert!(read(&d.path().join("missing")).is_none());
    }

    #[test]
    fn values_with_equals_are_skipped_and_later_keys_win() {
        let cfg = parse("a = 1\nb = x=y\na = 2\r\nc=3\rd = 4");
        assert_eq!(
            cfg,
            vec![
                ("a".to_owned(), "2".to_owned()),
                ("c".to_owned(), "3".to_owned()),
                ("d".to_owned(), "4".to_owned())
            ]
        );
    }

    /// Linear time (a 1 MiB file can hold ~110k distinct keys).
    #[test]
    fn many_keys_parse_quickly() {
        use std::fmt::Write as _;
        let mut text = String::new();
        for i in 0..100_000 {
            writeln!(text, "k{i} = v").unwrap();
        }
        let start = std::time::Instant::now();
        assert_eq!(parse(&text).len(), 100_000);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn version_forms() {
        assert!(versions_match("3.12.3.final.0", "3.12.3.final.0"));
        assert!(versions_match("3.12.3.final.0", "3.12.3"));
        assert!(!versions_match("3.12.4.final.0", "3.12.3"));
        assert!(!versions_match("3.12.3.final.0", "3.12.3.candidate.1"));
        assert!(!versions_match(
            "<<error retrieving version from x>>",
            "3.12.3"
        ));
        assert!(!versions_match("3.12", "3.12.3"));
        let cfg = parse("version = 3.13.1\n");
        assert_eq!(recorded_version(&cfg), Some("3.13.1"));
        let cfg = parse("version = 3.13.1\nversion_info = 3.13.1.final.0\n");
        assert_eq!(recorded_version(&cfg), Some("3.13.1.final.0"));
    }
}
