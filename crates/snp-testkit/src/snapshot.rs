//! `snp_snapshot!`: insta snapshots of an [`crate::Output`] with the [`crate::TestEnv`] filters (10 §7.3.4).
//!
//! Owner: WP-18.

use std::path::Path;

use crate::Output;

/// `regex::escape` (the `regex` crate is reserved for snp-pyre; insta compiles the filters).
fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The spellings of `p` that appear in output: native, `/`-separated (Windows git, `file://` URLs) and
/// JSON-escaped (`\\`).
fn spellings(p: &Path) -> Vec<String> {
    let native = p.to_string_lossy().into_owned();
    let mut v = vec![native.clone()];
    for alt in [native.replace('\\', "/"), native.replace('\\', "\\\\")] {
        if !v.contains(&alt) {
            v.push(alt);
        }
    }
    v
}

/// The insta filters of 10 §7.3.4, in application order (`(regex, replacement)`): the stores before the root, then
/// the volatile values.
pub(crate) fn filters(root: &Path) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = Vec::new();
    for store in ["store", "snp-store"] {
        for s in spellings(&root.join(store)) {
            f.push((escape(&s), "[STORE]".to_owned()));
        }
    }
    for s in spellings(root) {
        f.push((escape(&s), "[ROOT]".to_owned()));
    }
    for (re, rep) in [
        (r"patch\d{9,11}-\d+", "patch[TIME]-[PID]"),
        (r"- duration: \d+(\.\d+)?s", "- duration: [D]s"),
        (r"\[master [0-9a-f]{7}\]", "[master [SHORT]]"),
        (
            r"\[master \(root-commit\) [0-9a-f]{7}\]",
            "[master [SHORT]]",
        ),
        (
            r"(?m)^Merge made by the '(ort|recursive)' strategy\.$",
            "Merge made by the '[STRATEGY]' strategy.",
        ),
        (
            r"( 1 file changed, 0 insertions\(\+\), 0 deletions\(-\)| 0 files changed)",
            "[FILES_CHANGED]",
        ),
        ("repo[a-z0-9_]{8}", "repo[RAND]"),
        ("tmp[a-z0-9_]{8}", "tmp[RAND]"),
        (r"\r\n", "\n"),
        (r"\b(?:pre-commit|snp) \d+\.\d+\.\d+\b", "[VERSION]"),
    ] {
        f.push((re.to_owned(), rep.to_owned()));
    }
    f
}

/// The snapshot text of an [`Output`]: exit code, then stdout and stderr (lossy UTF-8).
#[doc(hidden)]
pub fn __render(out: &Output) -> String {
    format!(
        "exit_code: {}\n----- stdout -----\n{}\n----- stderr -----\n{}\n",
        out.code,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Snapshots `$out` (an [`crate::Output`]) with the filters of `$env` (a [`crate::TestEnv`]); `@"…"` makes it an
/// inline snapshot (preferred up to 30 lines, 10 §7.3.4).
#[macro_export]
macro_rules! snp_snapshot {
    ($env:expr, $out:expr) => {{
        let __text = $crate::__render(&$out);
        let mut __settings = $crate::__insta::Settings::clone_current();
        for (__re, __rep) in $crate::TestEnv::filters(&$env) {
            __settings.add_filter(&__re, __rep);
        }
        __settings.bind(|| {
            $crate::__insta::assert_snapshot!(__text);
        });
    }};
    ($env:expr, $out:expr, @$snapshot:literal) => {{
        let __text = $crate::__render(&$out);
        let mut __settings = $crate::__insta::Settings::clone_current();
        for (__re, __rep) in $crate::TestEnv::filters(&$env) {
            __settings.add_filter(&__re, __rep);
        }
        __settings.bind(|| {
            $crate::__insta::assert_snapshot!(__text, @$snapshot);
        });
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_regex_metacharacters() {
        assert_eq!(escape("/tmp/a.b+c(d)"), r"/tmp/a\.b\+c\(d\)");
        assert_eq!(escape(r"C:\x"), r"C:\\x");
    }

    #[test]
    fn renders_outputs() {
        let out = Output {
            code: 1,
            stdout: b"a\xff\n".to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            __render(&out),
            "exit_code: 1\n----- stdout -----\na\u{fffd}\n\n----- stderr -----\n\n"
        );
    }

    #[test]
    fn store_filters_come_before_the_root() {
        let f = filters(Path::new("/r/oot"));
        let store = Path::new("/r/oot").join("store");
        assert_eq!(
            f[0],
            (escape(&store.to_string_lossy()), "[STORE]".to_owned())
        );
        let root_at = f.iter().position(|(_, r)| r == "[ROOT]").unwrap();
        assert!(f.iter().take(root_at).all(|(_, r)| r == "[STORE]"));
    }
}
