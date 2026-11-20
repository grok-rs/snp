//! Name-based identification: `identify.tags_from_filename` and
//! `identify.tags_from_interpreter`, including Python's `os.path` rules.

use crate::extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};
use crate::generated::MAX_EXTENSION_LEN;
use crate::interpreters::INTERPRETERS;
use crate::table::hash_from;
use crate::tags::TagSet;

/// Identify a file from its name alone (no I/O), exactly like
/// `identify.tags_from_filename`.
///
/// `path` may contain directories; only the final component is used, split
/// with the platform's Python `os.path` rules (`/` on Unix; `/`, `\` and a
/// drive or UNC prefix on Windows, where, as in `ntpath.splitroot`, the
/// drive is any single character followed by `:`, e.g. `é:`). Bytes that
/// are not UTF-8 are fine: they simply never match a table key. On Windows
/// pass the WTF-8 bytes of an `OsStr` (`as_encoded_bytes`).
///
/// The rules, in order:
///
/// 1. the whole name, then each `.`-separated part, is looked up in
///    [`NAMES`]; the first hit is used (`Dockerfile.xenial` → `dockerfile`);
/// 2. the extension (`os.path.splitext`: the text after the last dot, unless
///    the name is only dots before it, so `.bashrc` has none) is lower-cased
///    and looked up in [`EXTENSIONS`], falling back to
///    [`EXTENSIONS_NEED_BINARY_CHECK`];
/// 3. both results are united (`README.md` → `markdown, plain-text, text`).
///
/// ```
/// use file_identify::tags_from_filename;
///
/// assert_eq!(tags_from_filename("src/setup.py").to_string(), "python, text");
/// assert_eq!(tags_from_filename("Dockerfile.xenial").to_string(), "dockerfile, text");
/// assert_eq!(tags_from_filename("photo.JPG").to_string(), "binary, image, jpeg");
/// assert!(tags_from_filename("unknown.xyz").is_empty());
/// ```
#[must_use]
pub fn tags_from_filename(path: impl AsRef<[u8]>) -> TagSet {
    tags_from_basename(basename(path.as_ref()))
}

/// [`tags_from_filename`] for a name without directory components.
///
/// One scan finds the first dot (a forward search up to it) and the last
/// dot (a backward search that stops at the first one); the name lookups
/// and the extension reuse them, and the whole name and its first part
/// share one hash pass ([`dotted_name_tags`]).
fn tags_from_basename(name: &[u8]) -> TagSet {
    let Some(first_dot) = first_dot_of(name) else {
        // No dot: `filename.split('.')` is `[filename]` and there is no
        // extension.
        return NAMES.get_bytes(name).unwrap_or_default();
    };
    let after_first = first_dot.saturating_add(1);
    let last_dot = name
        .get(after_first..)
        .and_then(|tail| tail.iter().rposition(|&b| b == b'.'))
        .map_or(first_dot, |i| i.saturating_add(after_first));
    let mut tags = dotted_name_tags(name, first_dot);
    if let Some(ext) = extension_at(name, last_dot) {
        tags = tags.union(extension_tags(ext));
    }
    tags
}

/// `name.iter().position(|&b| b == b'.')`, eight bytes at a time.
///
/// A byte of `x = word ^ DOTS` is zero exactly where `word` has a dot, and
/// `(x - LO) & !x & HI` sets the high bit of the lowest zero byte (a borrow
/// can only flag bytes above it), so the lowest set bit, counted from the
/// little-endian start of the word, is the first dot. The tail of fewer
/// than eight bytes is scanned bytewise. (Inlined into its caller, a plain
/// `position` loop costs about ten instructions per byte.)
fn first_dot_of(name: &[u8]) -> Option<usize> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    const DOTS: u64 = 0x2e2e_2e2e_2e2e_2e2e;
    let (words, tail) = name.as_chunks::<8>();
    let mut base = 0_usize;
    for &bytes in words {
        let x = u64::from_le_bytes(bytes) ^ DOTS;
        let found = x.wrapping_sub(LO) & !x & HI;
        if found != 0 {
            let lane = usize::try_from(found.trailing_zeros() >> 3).unwrap_or(0);
            return Some(base.saturating_add(lane));
        }
        base = base.saturating_add(8);
    }
    tail.iter()
        .position(|&b| b == b'.')
        .map(|i| base.saturating_add(i))
}

/// Tags for a shebang interpreter, exactly like
/// `identify.tags_from_interpreter`.
///
/// Everything up to the last `/` is dropped, then the name is looked up in
/// [`INTERPRETERS`], dropping one `.suffix` at a time until a match
/// (`python3.12.1` → `python3.12` → `python3`).
///
/// ```
/// use file_identify::tags_from_interpreter;
///
/// assert_eq!(tags_from_interpreter("/usr/bin/python3.12").to_string(), "python, python3");
/// assert!(tags_from_interpreter("/usr/bin/env").is_empty());
/// ```
#[must_use]
pub fn tags_from_interpreter(interpreter: &str) -> TagSet {
    // interpreter.rpartition('/')[2]
    let mut name = interpreter.rsplit('/').next().unwrap_or(interpreter);
    while !name.is_empty() {
        if let Some(tags) = INTERPRETERS.get(name) {
            return tags;
        }
        // interpreter.rpartition('.')[0]
        name = name.rsplit_once('.').map_or("", |(head, _)| head);
    }
    TagSet::EMPTY
}

/// `os.path.basename` for the current platform.
pub(crate) fn basename(path: &[u8]) -> &[u8] {
    if cfg!(windows) {
        nt_basename(path)
    } else {
        posix_basename(path)
    }
}

/// `posixpath.basename`: everything after the last `/`.
pub(crate) fn posix_basename(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(i) => path.get(i.saturating_add(1)..).unwrap_or_default(),
        None => path,
    }
}

const fn is_nt_sep(b: u8) -> bool {
    b == b'\\' || b == b'/'
}

/// `ntpath.basename` (`CPython` 3.12): strip the drive / UNC share and root
/// (`ntpath.splitroot`), then take everything after the last separator.
pub(crate) fn nt_basename(path: &[u8]) -> &[u8] {
    let rest = nt_after_root(path);
    match rest.iter().rposition(|&b| is_nt_sep(b)) {
        Some(i) => rest.get(i.saturating_add(1)..).unwrap_or_default(),
        None => rest,
    }
}

/// Length in bytes of the first character of `p`, counted the way Python
/// indexes the `str` (one code point per character).
///
/// Valid UTF-8 sequences and WTF-8 encoded surrogates (which Windows file
/// names may contain, and which Python sees as one lone-surrogate
/// character) count as one character; any other byte counts as one
/// character on its own, as with Python's `surrogateescape` decoding.
fn first_char_len(p: &[u8]) -> usize {
    let Some(&lead) = p.first() else {
        return 0;
    };
    let width = match lead {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        // ASCII, continuation bytes and bytes that never start a sequence.
        _ => return 1,
    };
    match p.get(..width) {
        Some(seq)
            if core::str::from_utf8(seq).is_ok()
                || matches!(seq, [0xed, 0xa0..=0xbf, 0x80..=0xbf]) =>
        {
            width
        }
        _ => 1,
    }
}

/// The third element of `ntpath.splitroot(path)` (the part after drive and
/// root).
///
/// Separators, `?` and the UNC prefix are ASCII, so byte positions equal
/// character positions everywhere except for the drive test, which looks
/// at the second *character*.
fn nt_after_root(p: &[u8]) -> &[u8] {
    let first = p.first().copied();
    let second = p.get(1).copied();
    if first.is_some_and(is_nt_sep) {
        if second.is_some_and(is_nt_sep) {
            // UNC drives (\\server\share, \\?\UNC\server\share) and device
            // drives (\\.\device, \\?\device).
            let unc_prefix = b"\\\\?\\UNC\\";
            let is_unc = p.len() >= unc_prefix.len()
                && p.iter().zip(unc_prefix).all(|(&a, &b)| {
                    let a = if a == b'/' {
                        b'\\'
                    } else {
                        a.to_ascii_uppercase()
                    };
                    a == b
                });
            let start = if is_unc { unc_prefix.len() } else { 2 };
            let find_sep = |from: usize| {
                p.get(from..)
                    .and_then(|tail| tail.iter().position(|&b| is_nt_sep(b)))
                    .map(|i| i.saturating_add(from))
            };
            let Some(index) = find_sep(start) else {
                return b"";
            };
            let Some(index2) = find_sep(index.saturating_add(1)) else {
                return b"";
            };
            p.get(index2.saturating_add(1)..).unwrap_or_default()
        } else {
            // Relative path with root, e.g. \Windows
            p.get(1..).unwrap_or_default()
        }
    } else {
        // `normp[1:2] == ':'`: the drive "letter" is any single character,
        // e.g. `é:x.py` has the drive `é:`.
        let colon = first_char_len(p);
        if p.get(colon) != Some(&b':') {
            // Relative path, e.g. Windows
            p
        } else if p
            .get(colon.saturating_add(1))
            .copied()
            .is_some_and(is_nt_sep)
        {
            // Absolute drive-letter path, e.g. X:\Windows
            p.get(colon.saturating_add(2)..).unwrap_or_default()
        } else {
            // Relative path with drive, e.g. X:Windows
            p.get(colon.saturating_add(1)..).unwrap_or_default()
        }
    }
}

/// `os.path.splitext(name)[1][1:]` for a name without separators: `None`
/// when there is no extension, `Some(b"")` for a trailing dot.
#[cfg(test)]
pub(crate) fn extension(name: &[u8]) -> Option<&[u8]> {
    extension_at(name, name.iter().rposition(|&b| b == b'.')?)
}

/// [`extension`] given the position of the last dot of `name`.
fn extension_at(name: &[u8], last_dot: usize) -> Option<&[u8]> {
    let (stem, ext) = name.split_at_checked(last_dot)?;
    // Leading dots of the name do not start an extension (".bashrc", "...").
    if stem.iter().all(|&b| b == b'.') {
        return None;
    }
    ext.get(1..)
}

/// `[filename] + filename.split('.')`, first hit in `NAMES`, for a name
/// whose first dot is at `first_dot`.
///
/// The whole name and its first part share one FNV pass: the state after
/// the first part is that part's hash, and hashing on from it over the rest
/// gives the whole name's. A whole name longer than any key is not hashed
/// past its first part; a first part longer than any key is not hashed at
/// all (the whole name is longer still). The later parts are looked up one
/// by one.
fn dotted_name_tags(name: &[u8], first_dot: usize) -> TagSet {
    let Some((first, rest)) = name.split_at_checked(first_dot) else {
        return TagSet::EMPTY;
    };
    if first.len() <= NAMES.max_len() {
        let first_hash = NAMES.hash(first);
        if name.len() <= NAMES.max_len() {
            let whole_hash = hash_from(first_hash, rest);
            if let Some(tags) = NAMES.get_hashed(name, whole_hash) {
                return tags;
            }
        }
        if let Some(tags) = NAMES.get_hashed(first, first_hash) {
            return tags;
        }
    }
    // `rest` starts with the first dot.
    rest.get(1..)
        .unwrap_or_default()
        .split(|&b| b == b'.')
        .find_map(|part| NAMES.get_bytes(part))
        .unwrap_or_default()
}

/// `ext.lower()` looked up in `EXTENSIONS`, else `EXTENSIONS_NEED_BINARY_CHECK`.
fn extension_tags(ext: &[u8]) -> TagSet {
    let lookup = |key: &[u8]| {
        EXTENSIONS
            .get_bytes(key)
            .or_else(|| EXTENSIONS_NEED_BINARY_CHECK.get_bytes(key))
    };
    let found = if ext.is_ascii() {
        // Fast path: lower-case into a stack buffer; anything longer than the
        // longest key cannot match.
        let mut buf = [0u8; MAX_EXTENSION_LEN];
        buf.get_mut(..ext.len()).and_then(|dst| {
            for (d, s) in dst.iter_mut().zip(ext) {
                *d = s.to_ascii_lowercase();
            }
            lookup(dst)
        })
    } else {
        // Python's str.lower() is a full Unicode mapping (e.g. U+212A KELVIN
        // SIGN lower-cases to ASCII 'k'). Invalid UTF-8 never matches.
        core::str::from_utf8(ext)
            .ok()
            .and_then(|s| lookup(s.to_lowercase().as_bytes()))
    };
    found.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn tags(name: &str) -> String {
        tags_from_filename(name).to_string()
    }

    /// The pre-U1 `tags_from_basename` (two dot scans, the first part
    /// hashed twice), kept as the differential reference.
    fn reference_tags_from_basename(name: &[u8]) -> TagSet {
        let mut tags = NAMES.get_bytes(name);
        if tags.is_none() && name.contains(&b'.') {
            tags = name.split(|&b| b == b'.').find_map(|p| NAMES.get_bytes(p));
        }
        let mut tags = tags.unwrap_or_default();
        let ext = name.iter().rposition(|&b| b == b'.').and_then(|dot| {
            let (stem, ext) = name.split_at_checked(dot).unwrap();
            (!stem.iter().all(|&b| b == b'.')).then(|| &ext[1..])
        });
        if let Some(ext) = ext {
            tags = tags.union(extension_tags(ext));
        }
        tags
    }

    fn assert_same_as_reference(name: &[u8]) {
        assert_eq!(
            tags_from_basename(name),
            reference_tags_from_basename(name),
            "{:?}",
            String::from_utf8_lossy(name)
        );
    }

    #[test]
    fn first_dot_matches_position() {
        // Every length up to five words, every dot position, and neighbours
        // of '.' (0x2d, 0x2f, 0xae = '.' | 0x80) and NUL around it.
        for len in 0..=40 {
            for filler in [b'a', b'-', b'/', 0xae, 0x00, 0xff] {
                let mut name = vec![filler; len];
                assert_eq!(first_dot_of(&name), None, "{name:?}");
                for first in 0..len {
                    name.fill(filler);
                    name[first] = b'.';
                    assert_eq!(first_dot_of(&name), Some(first), "{name:?}");
                    for second in first..len {
                        name[second] = b'.';
                        assert_eq!(first_dot_of(&name), Some(first), "{name:?}");
                        name[second] = filler;
                        name[first] = b'.';
                    }
                }
            }
        }
    }

    proptest! {
        #[test]
        fn first_dot_matches_position_on_arbitrary_bytes(
            name in proptest::collection::vec(any::<u8>(), 0..80)
        ) {
            prop_assert_eq!(first_dot_of(&name), name.iter().position(|&b| b == b'.'));
        }
    }

    #[test]
    fn single_scan_matches_reference_on_edge_cases() {
        let mut names: Vec<Vec<u8>> = [
            "",
            ".",
            "..",
            "...",
            ".bashrc",
            "a.",
            "..a",
            "..a.py",
            ".a.py",
            "a..b",
            "a.b.c.d",
            "Dockerfile",
            "Dockerfile.",
            ".Dockerfile",
            "x.Dockerfile.y",
            "Dockerfile.xenial",
            "xenial.Dockerfile",
            "setup.cfg",
            "Pipfile.lock",
            "Tiltfile.abc",
            ".pre-commit-config.yaml",
            "meson_options.txt",
            "a.b.c.Makefile.d.png",
            "Containerfile.hgrcasciidoc",
            "x.\u{212a}t",
            "x.\u{130}ni",
            "\u{212a}.Makefile",
            "Makefile.\u{212a}t",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        // Names around NAMES' longest key (17 bytes), with and without dots.
        for len in 15..=20 {
            names.push(vec![b'a'; len]);
            let mut first_long = vec![b'a'; len];
            first_long.extend_from_slice(b".Makefile.py");
            names.push(first_long);
            let mut whole_long = b"Makefile.".to_vec();
            whole_long.extend(std::iter::repeat_n(b'x', len));
            names.push(whole_long);
        }
        names.push(b"\xff\xfe.py".to_vec());
        names.push(b"Makefile.\xff".to_vec());
        names.push(b"\xff.Makefile".to_vec());
        names.push(b"x.p\xffy".to_vec());
        for name in &names {
            assert_same_as_reference(name);
        }
        // Every table key, alone, as a part and as an extension.
        for (key, _) in NAMES.iter().chain(EXTENSIONS.iter()) {
            for name in [
                key.to_owned(),
                format!("{key}.x"),
                format!("x.{key}"),
                format!(".{key}"),
                format!("{key}."),
                format!("x.{key}.y"),
                key.to_uppercase(),
            ] {
                assert_same_as_reference(name.as_bytes());
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4096))]

        /// Arbitrary bytes (non-UTF-8 included), dot-heavy names and
        /// names built from table keys.
        #[test]
        fn single_scan_matches_reference(
            name in prop_oneof![
                proptest::collection::vec(any::<u8>(), 0..64),
                proptest::collection::vec(
                    prop_oneof![Just(b'.'), Just(b'a'), Just(b'K'), Just(0xff_u8)],
                    0..40,
                ),
                proptest::collection::vec(
                    prop_oneof![
                        proptest::sample::select(NAMES.keys().collect::<Vec<_>>()),
                        proptest::sample::select(EXTENSIONS.keys().collect::<Vec<_>>()),
                        Just(""),
                        Just("x"),
                        Just("\u{212a}"),
                    ],
                    0..5,
                )
                .prop_map(|parts| parts.join(".").into_bytes()),
                "[.a-zA-Z0-9_\u{212a}é-]{0,300}".prop_map(String::into_bytes),
            ]
        ) {
            prop_assert_eq!(
                tags_from_basename(&name),
                reference_tags_from_basename(&name)
            );
        }
    }

    #[test]
    fn upstream_tags_from_filename_cases() {
        // identify/tests/identify_test.py::test_tags_from_filename
        for (name, expected) in [
            (".salt-lint", "salt-lint, text, yaml"),
            ("test.py", "python, text"),
            ("test.mk", "makefile, text"),
            ("Makefile", "makefile, text"),
            ("Containerfile", "dockerfile, text"),
            ("Dockerfile", "dockerfile, text"),
            ("Dockerfile.xenial", "dockerfile, text"),
            ("xenial.Dockerfile", "dockerfile, text"),
            ("Pipfile", "text, toml"),
            ("Pipfile.lock", "json, pipfile-lock, text"),
            ("poetry.lock", "poetry-lock, text, toml"),
            ("uv.lock", "text, toml, uv-lock"),
            ("mod/test.py", "python, text"),
            ("mod/Dockerfile", "dockerfile, text"),
            ("config.ru", "ruby, text"),
            ("Gemfile", "ruby, text"),
            ("Gemfile.lock", "gemfile-lock, text"),
            ("Jenkinsfile", "groovy, jenkins, text"),
            ("build.jenkins", "groovy, jenkins, text"),
            ("build.jenkinsfile", "groovy, jenkins, text"),
            ("meson.build", "meson, text"),
            ("meson.options", "meson, meson-options, text"),
            (
                "meson_options.txt",
                "meson, meson-options, plain-text, text",
            ),
            ("Vagrantfile", "ruby, text"),
            ("Tiltfile", "text, tiltfile"),
            ("Tiltfile.abc", "text, tiltfile"),
            ("test.Tiltfile", "text, tiltfile"),
            ("f.plist", "plist"),
            ("f.JPG", "binary, image, jpeg"),
            ("dockerfile.py", "python, text"),
            ("test.cfg", "text"),
            ("setup.cfg", "ini, text"),
            ("README.md", "markdown, plain-text, text"),
            ("test.weird-unrecognized-extension", ""),
            ("test", ""),
            ("", ""),
        ] {
            assert_eq!(tags(name), expected, "{name:?}");
        }
    }

    #[test]
    fn splitext_semantics() {
        assert_eq!(extension(b".bashrc"), None);
        assert_eq!(extension(b"..foo"), None);
        assert_eq!(extension(b"..."), None);
        assert_eq!(extension(b"foo."), Some(&b""[..]));
        assert_eq!(extension(b"a..b"), Some(&b"b"[..]));
        assert_eq!(extension(b".a.py"), Some(&b"py"[..]));
        assert_eq!(extension(b"noext"), None);
        assert_eq!(tags("..py"), "");
        assert_eq!(tags(".bashrc"), "bash, shell, text");
        assert_eq!(tags("Makefile.png"), "binary, image, makefile, png, text");
    }

    #[test]
    fn unicode_and_bytes() {
        // U+212A KELVIN SIGN lower-cases to ASCII 'k' in Python.
        assert_eq!(tags("x.\u{212a}t"), "kotlin, text");
        assert_eq!(tags("x.\u{130}ni"), "");
        assert!(tags_from_filename(b"\xff\xfe.py").contains("python"));
        assert!(tags_from_filename(b"x.p\xffy").is_empty());
        assert_eq!(tags("x.averyveryveryverylongextensionthatcannotmatch"), "");
    }

    #[test]
    fn posix_and_nt_basename() {
        assert_eq!(posix_basename(b"a/b/c.py"), b"c.py");
        assert_eq!(posix_basename(b"a\\b\\c.py"), b"a\\b\\c.py");
        assert_eq!(posix_basename(b"dir/"), b"");
        assert_eq!(nt_basename(b"a\\b/c.py"), b"c.py");
        assert_eq!(nt_basename(b"C:foo.py"), b"foo.py");
        assert_eq!(nt_basename(b"C:\\foo.py"), b"foo.py");
        assert_eq!(nt_basename(b"\\\\server\\share"), b"");
        assert_eq!(nt_basename(b"\\\\server\\share\\x.py"), b"x.py");
        assert_eq!(nt_basename(b"//server"), b"");
        assert_eq!(nt_basename(b"\\\\?\\UNC\\server\\share"), b"");
        assert_eq!(nt_basename(b"//?/unc/server/share/y.rs"), b"y.rs");
        assert_eq!(nt_basename(b"\\Windows"), b"Windows");
        assert_eq!(nt_basename(b""), b"");
    }

    #[test]
    fn nt_drive_is_one_character() {
        // CPython 3.12 ntpath.basename; the drive test looks at the second
        // character, not the second byte.
        for (path, expected) in [
            ("é:x.py", "x.py"),
            ("é:\\x.py", "x.py"),
            ("é:/x.py", "x.py"),
            ("€:a/b.rs", "b.rs"),
            ("😀:c.go", "c.go"),
            ("\u{212a}:Dockerfile", "Dockerfile"),
            ("é:", ""),
            ("::x.py", "x.py"),
            ("ab:c.py", "ab:c.py"),
            ("é\\:x.py", ":x.py"),
            ("éé:x.py", "éé:x.py"),
        ] {
            assert_eq!(
                nt_basename(path.as_bytes()),
                expected.as_bytes(),
                "{path:?}"
            );
        }
        // WTF-8 lone surrogate U+DCFF (a Windows name Python sees as '\udcff').
        assert_eq!(nt_basename(b"\xed\xb3\xbf:x.py"), b"x.py");
        // Undecodable bytes are one character each (surrogateescape).
        assert_eq!(nt_basename(b"\xff:x.py"), b"x.py");
        assert_eq!(nt_basename(b"\xc3:x.py"), b"x.py");
        assert_eq!(nt_basename(b"\xc3\xa9\xa9:x.py"), b"\xc3\xa9\xa9:x.py");
        // The reviewer's differential-fuzzing cases (identify with ntpath).
        let nt_tags = |p: &[u8]| tags_from_basename(nt_basename(p)).to_string();
        assert_eq!(
            nt_tags("é:Containerfile.hgrcasciidoc".as_bytes()),
            "dockerfile, text"
        );
        assert_eq!(nt_tags("é:bblayers.conf".as_bytes()), "bitbake, text");
        assert_eq!(nt_tags("é:.sqlfluff".as_bytes()), "ini, text");
    }

    #[test]
    fn first_char_lengths() {
        assert_eq!(first_char_len(b""), 0);
        assert_eq!(first_char_len(b"a:"), 1);
        assert_eq!(first_char_len("é:".as_bytes()), 2);
        assert_eq!(first_char_len("€".as_bytes()), 3);
        assert_eq!(first_char_len("😀".as_bytes()), 4);
        assert_eq!(first_char_len(b"\xed\xa0\x80"), 3); // WTF-8 surrogate
        assert_eq!(first_char_len(b"\xed\xa0"), 1); // truncated
        assert_eq!(first_char_len(b"\xe0\x80\x80"), 1); // overlong
        assert_eq!(first_char_len(b"\xf5\x80\x80\x80"), 1);
        assert_eq!(first_char_len(b"\x80"), 1);
        assert_eq!(first_char_len(b"\xf0\x9f\x98"), 1); // truncated
    }

    /// The parity vectors carry identify's result with `ntpath` whenever it
    /// differs from `posixpath`; check both columns on every platform (the
    /// integration test only checks the current platform's column).
    #[test]
    fn parity_vectors_for_both_path_flavours() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/identify-vectors.json"))
                .expect("valid vector file");
        let cases = data["tags_from_filename"].as_array().expect("cases");
        let strings = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .expect("array")
                .iter()
                .map(|s| s.as_str().expect("tag").to_owned())
                .collect()
        };
        let names = |t: TagSet| -> Vec<String> { t.names().map(str::to_owned).collect() };
        let mut nt_specific = 0;
        for case in cases {
            let name = case[0].as_str().expect("name").as_bytes();
            let posix = strings(&case[1]);
            let nt = case.get(2).map_or_else(|| posix.clone(), strings);
            nt_specific += usize::from(case.get(2).is_some());
            assert_eq!(names(tags_from_basename(posix_basename(name))), posix);
            assert_eq!(names(tags_from_basename(nt_basename(name))), nt);
        }
        assert!(nt_specific > 100, "{nt_specific}");
    }

    #[test]
    fn interpreters() {
        for (interp, expected) in [
            ("python", "python"),
            ("python3", "python, python3"),
            ("python3.5.2", "python, python3"),
            ("/usr/bin/python3.5.2", "python, python3"),
            ("/usr/bin/herpderpderpderpderp", ""),
            ("something-random", ""),
            ("", ""),
            (".python", ""),
            ("python.", "python"),
            ("/", ""),
            ("nu", "nushell"),
            ("escript", "erlang"),
        ] {
            assert_eq!(
                tags_from_interpreter(interp).to_string(),
                expected,
                "{interp:?}"
            );
        }
    }
}
