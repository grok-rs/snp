//! Catalog rows owned by WP-11 (docs/WBS.md "Catalog ownership", `tests/catalog/index.tsv`):
//! T10-XRG-16 (10 §5.4) and T10-UTL-07 (10 §5.18).
//!
//! Owner: WP-11.

#[cfg(test)]
mod catalog {
    use std::ffi::{OsStr, OsString};

    use snp_pycompat::{ntpath, ospath, posixpath, random};

    /// `lang_base._shuffled` (`lang_base.py:146-153`) over any sequence: the index permutation of
    /// `Random(1542676187).shuffle`.
    fn shuffled<'a>(seq: &[&'a str]) -> Vec<&'a str> {
        random::precommit_shuffled_indices(seq.len())
            .into_iter()
            .map(|i| seq[usize::try_from(i).unwrap()])
            .collect()
    }

    /// T10-XRG-16 (`tests/lang_base_test.py:138-141`): `_shuffled([str(i) for i in range(10)])`.
    #[test]
    fn t10_xrg_16_shuffled_is_deterministic() {
        let seq: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let seq: Vec<&str> = seq.iter().map(String::as_str).collect();
        assert_eq!(
            shuffled(&seq),
            ["4", "0", "5", "1", "8", "6", "2", "3", "7", "9"]
        );
        // a fresh generator per call: every hook shuffles the same way
        assert_eq!(shuffled(&seq), shuffled(&seq));
    }

    /// `Prefix(prefix_dir).path(*parts)` (`prefix.py:9-10`): `normpath(join(prefix_dir, *parts))`,
    /// in the POSIX flavour.
    fn prefix_path_posix(prefix: &str, parts: &[&str]) -> String {
        let parts: Vec<&[u8]> = parts.iter().map(|p| p.as_bytes()).collect();
        let joined = posixpath::join(prefix.as_bytes(), &parts);
        String::from_utf8(posixpath::normpath(&joined)).unwrap()
    }

    /// The same in the Windows flavour.
    fn prefix_path_nt(prefix: &str, parts: &[&str]) -> String {
        let parts: Vec<&[u8]> = parts.iter().map(|p| p.as_bytes()).collect();
        let joined = ntpath::join(prefix.as_bytes(), &parts);
        String::from_utf8(ntpath::normpath(&joined)).unwrap()
    }

    /// `norm_slash` of `prefix_test.py`: `/` → `os.sep` of the flavour.
    fn nt_slash(s: &str) -> String {
        s.replace('/', "\\")
    }

    /// T10-UTL-07 (`tests/prefix_test.py:20-46`): `Prefix.path` normalises the joined path; an
    /// absolute second part wins; several parts join like `os.path.join`; `star` lists the matching
    /// basenames.
    #[test]
    fn t10_utl_07_prefix_path_normpath_join() {
        let cases = [
            ("foo", "", "foo"),
            ("foo", "bar", "foo/bar"),
            ("foo/bar", "../baz", "foo/baz"),
            ("./", "bar", "bar"),
            ("./", "", "."),
            ("/tmp/foo", "/tmp/bar", "/tmp/bar"),
        ];
        for (prefix, part, want) in cases {
            assert_eq!(
                prefix_path_posix(prefix, &[part]),
                want,
                "{prefix:?} {part:?}"
            );
            assert_eq!(
                prefix_path_nt(&nt_slash(prefix), &[&nt_slash(part)]),
                nt_slash(want),
                "nt {prefix:?} {part:?}"
            );
            // the host facade (what snp-lang's Prefix calls)
            let sep = if cfg!(windows) { "\\" } else { "/" };
            let host = |s: &str| s.replace('/', sep);
            let joined = ospath::join(OsStr::new(&host(prefix)), &[OsStr::new(&host(part))]);
            assert_eq!(
                ospath::normpath(&joined),
                OsString::from(host(want)),
                "host {prefix:?} {part:?}"
            );
        }
        // test_path_multiple_args: Prefix('foo').path('bar', 'baz') == os.path.join('foo', 'bar',
        // 'baz')
        assert_eq!(prefix_path_posix("foo", &["bar", "baz"]), "foo/bar/baz");
        assert_eq!(
            String::from_utf8(posixpath::join(b"foo", &[b"bar", b"baz"])).unwrap(),
            "foo/bar/baz"
        );
        assert_eq!(prefix_path_nt("foo", &["bar", "baz"]), "foo\\bar\\baz");
    }

    // T10-UTL-07 `test_star` (`prefix_test.py:43-46`, `Prefix.star` = `tuple(p for p in
    // os.listdir(prefix_dir) if p.endswith(end))`) calls no `os.path` function: it is a plain
    // directory listing with a suffix test and belongs to snp-lang's `Prefix`, not to this crate.
}
