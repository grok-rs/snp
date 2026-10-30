//! Property tests of 15 §A7 and the invariants the consumers rely on, over seeded random inputs
//! drawn from alphabets rich in every boundary class (PY-WS, PY-LB, B-WS, quotes, separators,
//! surrogate-escaped bytes). The crate has no dependencies, not even dev-dependencies
//! (`cargo tree -p snp-pycompat` stays empty), so the generator is a local `SplitMix64`.
//!
//! Owner: WP-11.

#[cfg(test)]
mod properties {
    use std::ffi::OsStr;

    use snp_pycompat::chars::{PyChar, is_bytes_space, is_linebreak, is_space};
    use snp_pycompat::{
        codec, eaw, float, int, ntpath, ospath, posixpath, precommit, random, repr, shlex, text,
        textio,
    };

    /// `SplitMix64`: a tiny, deterministic generator.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
        }
        fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[self.below(xs.len())]
        }
        /// A string of up to `max` pieces from `alphabet`.
        fn text(&mut self, alphabet: &[&str], max: usize) -> String {
            let n = self.below(max + 1);
            (0..n).map(|_| *self.pick(alphabet)).collect()
        }
        fn bytes(&mut self, alphabet: &[&[u8]], max: usize) -> Vec<u8> {
            let n = self.below(max + 1);
            (0..n)
                .flat_map(|_| self.pick(alphabet).iter().copied())
                .collect()
        }
    }

    const ROUNDS: usize = 3000;

    /// Every whitespace / line-break / quote class of 15 §A3, plus ordinary text.
    const TEXT: &[&str] = &[
        "a",
        "b",
        "Z",
        "0",
        " ",
        "  ",
        "\t",
        "\n",
        "\r",
        "\r\n",
        "\x0b",
        "\x0c",
        "\x1c",
        "\x1d",
        "\x1e",
        "\x1f",
        "\u{85}",
        "\u{a0}",
        "\u{1680}",
        "\u{2000}",
        "\u{200a}",
        "\u{200b}",
        "\u{2028}",
        "\u{2029}",
        "\u{202f}",
        "\u{205f}",
        "\u{3000}",
        "\u{feff}",
        "\u{180e}",
        "#",
        ",",
        ":",
        "'",
        "\"",
        "\\",
        "$",
        "\x00",
        "\x7f",
        "é",
        "😀",
        ".",
        "/",
        "\u{145}",
        "\u{2018}",
        "\u{2828}",
        "rev:",
        "# frozen: v1",
        "Σ",
    ];

    const BYTES: &[&[u8]] = &[
        b"a",
        b" ",
        b"\t",
        b"\n",
        b"\r",
        b"\x0b",
        b"\x0c",
        b"\x1c",
        b"\x1f",
        b"\x00",
        b"#",
        b",",
        b"\x80",
        b"\x85",
        b"\xa0",
        b"\xc2",
        b"\xc2\x85",
        b"\xc2\xa0",
        b"\xe2\x80\xa8",
        b"\xe3\x80\x80",
        b"\xe9",
        b"\xed\xa0\x80",
        b"\xf0\x9f\x98\x80",
        b"\xf4\x90",
        b"\xff",
        b"\xef\xbb\xbf",
    ];

    const POSIX: &[&str] = &["a", "b", ".", "..", "", "~", "x.y", ".z", "/", "//", "é"];

    const NT: &[&str] = &[
        "a",
        "B",
        ".",
        "..",
        "",
        "x.y",
        "\\",
        "/",
        "C:",
        "c:",
        "D:",
        "\\\\srv\\sh",
        "//srv/sh",
        "\\\\?\\",
        "UNC",
        "é",
        "É",
        "Σ",
        "😀",
        ":",
    ];

    #[test]
    fn shlex_split_of_quote_is_identity() {
        let mut rng = Rng(1);
        for _ in 0..ROUNDS {
            let s = rng.text(TEXT, 8);
            let q = shlex::quote(&s);
            assert_eq!(shlex::split(&q), Ok(vec![s.clone()]), "{s:?} -> {q:?}");
            assert_eq!(
                &*shlex::quote_bytes(s.as_bytes()),
                q.as_bytes(),
                "quote_bytes agrees on text: {s:?}"
            );
        }
        for _ in 0..ROUNDS / 4 {
            let args: Vec<String> = (0..rng.below(5)).map(|_| rng.text(TEXT, 4)).collect();
            assert_eq!(
                shlex::split(&shlex::join(&args)),
                Ok(args.clone()),
                "{args:?}"
            );
        }
    }

    #[test]
    fn shlex_split_never_panics_and_tokens_are_consistent() {
        let mut rng = Rng(2);
        for _ in 0..ROUNDS {
            let s = rng.text(TEXT, 12);
            if let Ok(tokens) = shlex::split(&s) {
                // re-quoting the tokens round-trips through split
                assert_eq!(
                    shlex::split(&shlex::join(&tokens)),
                    Ok(tokens.clone()),
                    "{s:?}"
                );
            }
        }
    }

    #[test]
    fn whitespace_split_properties() {
        let mut rng = Rng(3);
        for _ in 0..ROUNDS {
            let s = rng.text(TEXT, 12);
            let all: Vec<&str> = text::split_whitespace(&s, None).collect();
            for item in &all {
                assert!(!item.is_empty() && !item.chars().any(is_space), "{s:?}");
            }
            assert_eq!(text::rsplit_whitespace(&s, None), all, "{s:?}");
            for n in 0..5 {
                let left: Vec<&str> = text::split_whitespace(&s, Some(n)).collect();
                let right = text::rsplit_whitespace(&s, Some(n));
                assert!(left.len() <= n + 1 && right.len() <= n + 1, "{s:?}");
                // below the limit the split never keeps a remainder's whitespace
                if all.len() <= n {
                    assert_eq!(left, all, "{s:?}");
                    assert_eq!(right, all, "{s:?}");
                }
            }
            match text::rsplit4(&s) {
                Ok(f) => assert_eq!(f.to_vec(), text::rsplit_whitespace(&s, Some(3)), "{s:?}"),
                Err(e) => {
                    assert_eq!(e.got, all.len(), "{s:?}");
                    assert!(e.got < 4);
                }
            }
            assert_eq!(text::strip(&s), text::lstrip(text::rstrip(&s)), "{s:?}");
        }
    }

    #[test]
    fn bytes_whitespace_properties() {
        let mut rng = Rng(4);
        for _ in 0..ROUNDS {
            let b = rng.bytes(BYTES, 12);
            let all = text::bytes_rsplit_whitespace(&b, None);
            for item in &all {
                assert!(!item.is_empty() && !item.iter().copied().any(is_bytes_space));
            }
            match text::bytes_rsplit4(&b) {
                Ok(f) => assert_eq!(f.to_vec(), text::bytes_rsplit_whitespace(&b, Some(3))),
                Err(e) => assert_eq!(e.got, all.len()),
            }
            assert_eq!(
                text::bytes_strip(&b),
                text::bytes_lstrip(text::bytes_rstrip(&b))
            );
            let fs = text::strip_fsbytes(&b);
            if let Ok(st) = std::str::from_utf8(&b) {
                assert_eq!(fs, text::strip(st).as_bytes(), "{b:?}");
            }
        }
    }

    #[test]
    fn splitlines_round_trips() {
        let mut rng = Rng(5);
        for _ in 0..ROUNDS {
            let s = rng.text(TEXT, 14);
            let keep: Vec<&str> = text::splitlines_keepends(&s).collect();
            assert_eq!(keep.concat(), s, "{s:?}");
            let plain: Vec<&str> = text::splitlines(&s).collect();
            assert_eq!(plain.len(), keep.len());
            for (p, k) in plain.iter().zip(&keep) {
                assert!(k.starts_with(p) && !p.chars().any(is_linebreak), "{s:?}");
            }
            let mut back: Vec<&str> = text::splitlines_keepends(&s).rev().collect();
            back.reverse();
            assert_eq!(back, keep, "{s:?}");
            let mut back_plain: Vec<&str> = text::splitlines(&s).rev().collect();
            back_plain.reverse();
            assert_eq!(back_plain, plain, "{s:?}");

            let b = rng.bytes(BYTES, 14);
            let bkeep: Vec<&[u8]> = text::bytes_splitlines_keepends(&b).collect();
            assert_eq!(bkeep.concat(), b);
            let mut bback: Vec<&[u8]> = text::bytes_splitlines_keepends(&b).rev().collect();
            bback.reverse();
            assert_eq!(bback, bkeep);
            for line in text::bytes_splitlines(&b) {
                assert!(!line.contains(&b'\n') && !line.contains(&b'\r'));
            }
        }
    }

    #[test]
    fn codec_properties() {
        let mut rng = Rng(6);
        for _ in 0..ROUNDS {
            let b = rng.bytes(BYTES, 10);
            match (codec::decode_utf8(&b), std::str::from_utf8(&b)) {
                (Ok(a), Ok(c)) => assert_eq!(a, c),
                (Err(e), Err(std_err)) => {
                    assert_eq!(e.start, std_err.valid_up_to());
                    assert!(e.end > e.start && e.end <= b.len());
                    assert_eq!(Some(&e.first_byte), b.get(e.start));
                    assert!(e.to_string().starts_with("'utf-8' codec can't decode byte"));
                }
                (a, c) => panic!("{b:?}: {a:?} vs {c:?}"),
            }
            // the surrogateescape view re-encodes to the same bytes
            let mut back = Vec::new();
            for c in codec::fsdecode_chars(&b) {
                match c {
                    PyChar::Scalar(c) => {
                        let mut buf = [0_u8; 4];
                        back.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    PyChar::Surrogate(u) => {
                        assert!((0xdc80..=0xdcff).contains(&u));
                        back.push(u8::try_from(u - 0xdc00).unwrap());
                    }
                }
            }
            assert_eq!(back, b);
            let br = codec::decode_utf8_backslashreplace(&b);
            if let Ok(st) = std::str::from_utf8(&b) {
                assert_eq!(br, st);
                assert_eq!(repr::repr_fsbytes(&b), repr::repr_str(st));
            } else {
                assert!(br.contains("\\x"));
                assert!(repr::repr_fsbytes(&b).contains("\\udc"));
            }
        }
    }

    #[test]
    fn posixpath_laws() {
        let mut rng = Rng(7);
        let cwd = b"/home/u/repo/sub";
        for _ in 0..ROUNDS {
            let parts: Vec<&str> = (0..rng.below(6)).map(|_| *rng.pick(POSIX)).collect();
            let p = parts.join("/");
            let n = posixpath::normpath(p.as_bytes());
            assert_eq!(posixpath::normpath(&n), n, "{p:?}");
            assert_eq!(
                posixpath::abspath(p.as_bytes(), cwd),
                posixpath::normpath(&posixpath::join(cwd, &[p.as_bytes()])),
                "{p:?}"
            );
            if !posixpath::isabs(p.as_bytes()) && !n.starts_with(b"..") {
                let abs = posixpath::abspath(p.as_bytes(), cwd);
                assert_eq!(
                    posixpath::relpath(&abs, Some(cwd), cwd),
                    Ok(n.clone()),
                    "{p:?}"
                );
            }
            let (head, tail) = posixpath::split(p.as_bytes());
            assert!(!tail.contains(&b'/'));
            let (root, ext) = posixpath::splitext(p.as_bytes());
            assert_eq!(<[&[u8]; 2]>::from((root, ext)).concat(), p.as_bytes());
            assert!(head.len() + tail.len() <= p.len());
            // the host facade is posixpath on Unix and ntpath over UTF-16 on Windows
            let host = ospath::normpath(OsStr::new(&p));
            #[cfg(unix)]
            assert_eq!(host.as_encoded_bytes(), &n[..]);
            #[cfg(windows)]
            assert_eq!(
                host.to_str().unwrap(),
                String::from_utf8(ntpath::normpath(p.as_bytes())).unwrap()
            );
        }
    }

    #[test]
    fn ntpath_laws_and_unit_agreement() {
        let mut rng = Rng(8);
        let cwd = "C:\\Users\\u\\repo\\sub";
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().collect() };
        let narrow = |w: &[u16]| String::from_utf16(w).unwrap();
        let text8 = |v: &[u8]| String::from_utf8(v.to_vec()).unwrap();
        for _ in 0..ROUNDS {
            let a: String = (0..rng.below(6))
                .map(|_| *rng.pick(NT))
                .collect::<Vec<_>>()
                .join("\\");
            let b: String = (0..rng.below(6))
                .map(|_| *rng.pick(NT))
                .collect::<Vec<_>>()
                .join("/");
            let n = ntpath::normpath(a.as_bytes());
            // idempotent unless `..` uncovered a drive-like component (as in CPython:
            // normpath('x\\..\\.:.') is '.:.', whose own normpath is '.:')
            let (da, ra, _) = ntpath::splitroot(a.as_bytes());
            let (dn, rn, _) = ntpath::splitroot(&n);
            if da.len() == dn.len() && ra.len() == rn.len() {
                assert_eq!(ntpath::normpath(&n), n, "{a:?}");
            }
            // the same results over UTF-8 and UTF-16 units
            assert_eq!(text8(&n), narrow(&ntpath::normpath(&wide(&a))), "{a:?}");
            assert_eq!(
                text8(&ntpath::join(a.as_bytes(), &[b.as_bytes()])),
                narrow(&ntpath::join(&wide(&a), &[&wide(&b)])),
                "{a:?} {b:?}"
            );
            let (d8, r8, t8) = ntpath::splitroot(a.as_bytes());
            let wa = wide(&a);
            let (d16, r16, t16) = ntpath::splitroot(&wa);
            assert_eq!(
                (text8(d8), text8(r8), text8(t8)),
                (narrow(d16), narrow(r16), narrow(t16)),
                "{a:?}"
            );
            assert_eq!(
                text8(&ntpath::normcase(a.as_bytes())),
                narrow(&ntpath::normcase(&wa))
            );
            assert_eq!(ntpath::isabs_312(a.as_bytes()), ntpath::isabs_312(&wa));
            assert_eq!(ntpath::isabs_313(a.as_bytes()), ntpath::isabs_313(&wa));
            let rel8 = ntpath::relpath(a.as_bytes(), Some(b.as_bytes()), cwd.as_bytes())
                .map(|v| text8(&v));
            let rel16 = ntpath::relpath(&wa, Some(&wide(&b)), &wide(cwd)).map(|v| narrow(&v));
            assert_eq!(rel8, rel16, "{a:?} {b:?}");
            let c8 = ntpath::commonpath(&[a.as_bytes(), b.as_bytes()]).map(|v| text8(&v));
            let c16 = ntpath::commonpath(&[&wa, &wide(&b)]).map(|v| narrow(&v));
            assert_eq!(c8, c16, "{a:?} {b:?}");
            let (root, ext) = ntpath::splitext(a.as_bytes());
            assert_eq!(<[&[u8]; 2]>::from((root, ext)).concat(), a.as_bytes());
            let (drive, rest) = ntpath::splitdrive(a.as_bytes());
            assert_eq!(<[&[u8]; 2]>::from((drive, rest)).concat(), a.as_bytes());
            // relpath of an absolute spelling of a relative path is the normalised path
            let plain = !a.contains(':') && !a.starts_with(['\\', '/']);
            if plain && !n.starts_with(b"..") {
                let abs = ntpath::abspath_lexical(a.as_bytes(), cwd.as_bytes());
                assert_eq!(
                    ntpath::relpath(&abs, Some(cwd.as_bytes()), cwd.as_bytes()).map(|v| text8(&v)),
                    Ok(text8(&n)),
                    "{a:?}"
                );
            }
        }
    }

    #[test]
    fn composites_properties() {
        let mut rng = Rng(9);
        for _ in 0..ROUNDS {
            let raw = rng.bytes(&[b"a", b"\0", b"\0\0", b" ", b"\xff", b"\n"], 10);
            let items: Vec<&[u8]> = precommit::zsplit(&raw).collect();
            let start = raw.iter().take_while(|&&c| c == 0).count();
            let end = raw.len() - raw.iter().rev().take_while(|&&c| c == 0).count();
            let core = if start < end {
                &raw[start..end]
            } else {
                &raw[..0]
            };
            assert_eq!(items.join(&0_u8), core, "{raw:?}");
            assert_eq!(items.is_empty(), core.is_empty());

            let sample = rng.text(TEXT, 12);
            let (head, rest) = precommit::migrate_header_split(&sample);
            assert_eq!(format!("{head}{rest}"), sample);
            assert_eq!(textio::textwrap_indent(&sample, ""), sample);
            let u = textio::universal_newlines(&sample);
            assert!(!u.contains('\r'));
            assert_eq!(
                textio::windows_text_write(&u).replace("\r\n", "\n"),
                u.as_ref()
            );
            if let Some(g) = precommit::rev_line_match(&sample) {
                let rebuilt = format!("{}rev:{}{}{}{}{}", g[0], g[1], g[2], g[3], g[4], g[5]);
                assert!(
                    sample == rebuilt || sample == format!("{rebuilt}\n"),
                    "{sample:?}"
                );
                assert!(!g[0].is_empty() && !g[3].is_empty() && g[5] == "\n");
                let same = precommit::rev_line_rewrite(&sample, &format!("{}{}", g[2], g[3]), None);
                assert!(same.is_some());
            }
            assert!(
                precommit::pre_push_lines(&sample).count() <= text::splitlines(&sample).count()
            );
            let skip = precommit::get_skips(Some(OsStr::new(&sample)));
            for item in &skip {
                assert!(!item.is_empty());
                let item = item.to_str().unwrap();
                assert_eq!(text::strip(item), item);
                assert!(!item.contains(','));
            }
            let msg = rng.bytes(
                &[b"\t", b"#\t", b"#", b"x", b" ", b"\n", b"\r\n", b"\x0b"],
                10,
            );
            for line in precommit::merge_msg_conflicts(&msg) {
                assert_eq!(text::bytes_strip(line), line);
            }
        }
    }

    #[test]
    fn numeric_properties() {
        let mut rng = Rng(10);
        for _ in 0..ROUNDS {
            let int_value = i128::from(rng.next().cast_signed()) * i128::from(rng.next() % 1000);
            assert_eq!(int::parse_int(&int_value.to_string()), Ok(int_value));
            let spaced = format!("\u{a0} {int_value}\t\n");
            assert_eq!(int::parse_int(&spaced), Ok(int_value));
            let float_value = f64::from_bits(rng.next());
            if float_value.is_finite() {
                let rep = float::repr(float_value);
                assert_eq!(rep.parse::<f64>().ok(), Some(float_value), "{rep}");
                let dur = float::format_duration(float_value);
                if dur != "0" {
                    let back: f64 = dur.parse().unwrap();
                    let bound = 0.005_000_1 * float_value.abs().max(1.0);
                    assert!((back - float_value).abs() <= bound, "{dur}");
                }
            }
            let len = rng.below(300);
            let mut perm = random::precommit_shuffled_indices(len);
            perm.sort_unstable();
            let identity: Vec<u32> = (0..u32::try_from(len).unwrap()).collect();
            assert_eq!(perm, identity);
            let mut mt = random::Mt19937::init_by_array(&[u32::try_from(len).unwrap()]);
            let bound = rng.next() | 1;
            assert!(mt.randbelow(bound) < bound);
            let sample = rng.text(TEXT, 10);
            let width = eaw::len_cjk(&sample);
            let count = sample.chars().count();
            assert!(count <= width && width <= 2 * count);
        }
    }
}
