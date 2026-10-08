//! The WP-11 extension sections (after `decimal_zeros` in the 3.12 file): MT19937 and the
//! deterministic shuffle, `round(x, 2)` and `repr(float)`, east-asian widths, `int()`, signal
//! names, `repr` of code points, more `ntpath` roots and case folding, random text-mode,
//! split and bytes-split corpora, and the bytes pre-push parse.

use std::ffi::OsStr;

use snp_pycompat::{chars, eaw, float, int, ntpath, precommit, random, repr, text, textio};

use crate::common::{
    V, by, f64_of_bits, hex, hexes, in_ranges, outcome, py_str, ranges, repr_of, s, sec, strs, u8s,
    usize_of, view313, want,
};
use crate::support::json::Value;

/// `_fnv1a64(','.join(map(str, seq)).encode())` of the generator.
fn fnv1a64(seq: &[u32]) -> String {
    let joined = seq.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in joined.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[test]
fn mt19937_seeding_and_output() {
    for e in sec("random_mt19937") {
        let key: Vec<u32> = e["key"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| u32::try_from(k.as_u64().unwrap()).unwrap())
            .collect();
        let mut rng = random::Mt19937::init_by_array(&key);
        let got: Vec<u64> = (0..8).map(|_| u64::from(rng.genrand_u32())).collect();
        let want: Vec<u64> = e["genrand"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap())
            .collect();
        assert_eq!(got, want, "{e}");
    }
    // `random.seed(0)` is `init_by_array([0])`: the empty key behaves the same
    let mut a = random::Mt19937::init_by_array(&[]);
    let mut b = random::Mt19937::init_by_array(&[0]);
    assert_eq!(a.genrand_u32(), b.genrand_u32());
}

#[test]
fn randbelow_matches_cpython() {
    let rb = &V["random_randbelow"];
    let seed = u32::try_from(rb["seed"].as_u64().unwrap()).unwrap();
    let mut rng = random::Mt19937::init_by_array(&[seed]);
    for case in rb["cases"].as_array().unwrap() {
        let n = case[0].as_u64().unwrap();
        for w in case[1].as_array().unwrap() {
            assert_eq!(rng.randbelow(n), w.as_u64().unwrap(), "randbelow({n})");
        }
    }
}

/// Acceptance: `precommit_shuffled_indices` equals CPython for n ∈ {0..64, 100, 1000, 2000, 5000}
/// and every n ≤ 2000 step 37.
#[test]
fn shuffle_matches_cpython() {
    let entries = sec("random_shuffle");
    let mut ns: Vec<usize> = entries.iter().map(|e| usize_of(&e["n"])).collect();
    for n in (0..=64)
        .chain([100, 1000, 2000, 5000])
        .chain((0..=2000).filter(|n| n % 37 == 0))
    {
        assert!(ns.contains(&n), "n = {n} missing from the vectors");
    }
    ns.dedup();
    assert_eq!(ns.len(), entries.len());
    for e in entries {
        let n = usize_of(&e["n"]);
        let got = random::precommit_shuffled_indices(n);
        assert_eq!(got.len(), n);
        if let Some(out) = e["out"].as_array() {
            let want: Vec<u32> = out
                .iter()
                .map(|x| u32::try_from(x.as_u64().unwrap()).unwrap())
                .collect();
            assert_eq!(got, want, "n = {n}");
        }
        assert_eq!(fnv1a64(&got), s(&e["fnv1a64"]), "n = {n}");
    }
}

#[test]
fn shuffle_of_strings() {
    let e = &V["random_shuffle_strings"];
    let input = strs(&e["in"]);
    let order = random::precommit_shuffled_indices(input.len());
    let got: Vec<&str> = order
        .iter()
        .map(|&i| input[usize::try_from(i).unwrap()])
        .collect();
    assert_eq!(got, strs(&e["out"]));
}

/// Acceptance: `format_duration` equals Python on 10,000 random f64 plus ties.
#[test]
fn format_duration_matches_round2() {
    let entries = sec("float_round2");
    assert!(entries.len() >= 10_000 + 2_000);
    for e in entries {
        let (bits, want) = s(e).split_once(' ').unwrap();
        let x = f64_of_bits(bits);
        assert_eq!(float::format_duration(x), want, "{x:e} ({bits})");
    }
    for (x, want) in [(0.125, "0.12"), (2.675, "2.67"), (0.0, "0"), (-0.0, "0")] {
        assert_eq!(float::format_duration(x), want);
    }
}

#[test]
fn float_repr_matches_cpython() {
    for e in sec("float_repr") {
        let (bits, want) = s(e).split_once(' ').unwrap();
        let x = f64_of_bits(bits);
        assert_eq!(float::repr(x), want, "{bits}");
        assert_eq!(repr::repr_float(x), want, "{bits}");
        assert_eq!(repr::repr(&repr::PyValue::Float(x)), want, "{bits}");
    }
}

/// Acceptance: `len_cjk` equals CPython 3.13 `east_asian_width` over all code points.
#[test]
fn east_asian_width_over_all_code_points() {
    let wide = ranges(&view313("east_asian_width_wf"));
    let mut buf = [0_u8; 4];
    for cp in 0..=0x10_ffff_u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let want = if in_ranges(cp, &wide) { 2 } else { 1 };
        assert_eq!(eaw::len_cjk(c.encode_utf8(&mut buf)), want, "U+{cp:04X}");
    }
    for e in view313("len_cjk").as_array().unwrap() {
        assert_eq!(eaw::len_cjk(s(&e["in"])), usize_of(&e["out"]), "{e}");
    }
}

#[test]
fn int_parse_matches_cpython() {
    for e in sec("int_parse") {
        let lit = py_str(&e["in"]).text();
        match (int::parse_int(&lit), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => assert_eq!(Some(g), w.as_i128(), "{e}"),
            (Err(g), Err((t, m))) => {
                assert_eq!(g.to_string(), m, "{e}");
                assert_eq!(g.py_type(), t, "{e}");
                assert_eq!(repr::int_literal_error(&lit), m, "{e}");
                assert_eq!(g.literal, lit);
            }
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn signal_names_match_linux_cpython() {
    let sn = &V["signal_names"];
    assert_eq!(s(&sn["platform"]), "linux");
    for pair in sn["names"].as_array().unwrap() {
        let n = u32::try_from(pair[0].as_u64().unwrap()).unwrap();
        assert_eq!(repr::signal_name(n), pair[1].as_str(), "signal {n}");
    }
}

/// `repr()` of single code points around every printability class, in the UCD 15.1 view.
#[test]
fn repr_of_code_points() {
    for e in view313("repr_codepoints").as_array().unwrap() {
        assert_eq!(repr_of(&e["in"]), s(&e["out"]), "{e}");
        if let Some(t) = e["in"].get("v").and_then(Value::as_str) {
            let c = t.chars().next().unwrap();
            let escaped = s(&e["out"]).starts_with("'\\") && t != "\\" && t != "'";
            if !c.is_ascii() {
                assert_eq!(chars::is_printable(c), !escaped, "{e}");
            }
        }
    }
}

#[test]
fn ntpath_roots_and_case_folding() {
    for e in sec("ntpath_more") {
        let p = by(s(&e["in"]));
        let (drive, root, tail) = ntpath::splitroot(p);
        assert_eq!(
            vec![u8s(drive), u8s(root), u8s(tail)],
            strs(&e["splitroot"]),
            "{e}"
        );
        assert_eq!(u8s(&ntpath::normpath(p)), s(&e["normpath"]), "{e}");
        let (head, tail) = ntpath::split(p);
        assert_eq!(vec![u8s(&head), u8s(tail)], strs(&e["split"]), "{e}");
        let (root, ext) = ntpath::splitext(p);
        assert_eq!(vec![u8s(root), u8s(ext)], strs(&e["splitext"]), "{e}");
        let (dr, rest) = ntpath::splitdrive(p);
        assert_eq!(vec![u8s(dr), u8s(rest)], strs(&e["splitdrive"]), "{e}");
        assert_eq!(ntpath::isabs_312(p), e["isabs"].as_bool().unwrap(), "{e}");
        assert_eq!(u8s(&ntpath::normcase(p)), s(&e["normcase"]), "{e}");
        let env = |k: &str| match k {
            "USERPROFILE" => Some("C:\\Users\\\u{c9}lodie".as_bytes().to_vec()),
            "USERNAME" => Some("\u{c9}lodie".as_bytes().to_vec()),
            _ => None,
        };
        let tilde = format!("~{}", s(&e["in"]));
        assert_eq!(
            u8s(&ntpath::expanduser(tilde.as_bytes(), &env)),
            s(&e["expanduser_profile"]),
            "{e}"
        );
        // the same answers over UTF-16 units
        let wide: Vec<u16> = s(&e["in"]).encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&ntpath::normpath(&wide)).unwrap(),
            s(&e["normpath"]),
            "{e}"
        );
        assert_eq!(
            String::from_utf16(&ntpath::normcase(&wide)).unwrap(),
            s(&e["normcase"]),
            "{e}"
        );
        assert_eq!(
            ntpath::isabs_312(&wide),
            e["isabs"].as_bool().unwrap(),
            "{e}"
        );
    }
    for e in sec("ntpath_pairs") {
        let (a, b, cwd) = (by(s(&e["a"])), by(s(&e["b"])), by(s(&e["cwd"])));
        assert_eq!(u8s(&ntpath::join(a, &[b])), s(&e["join"]), "{e}");
        assert_eq!(u8s(&ntpath::join(b, &[a])), s(&e["join_rev"]), "{e}");
        assert_eq!(
            ntpath::relpath(a, Some(b), cwd)
                .map(|v| u8s(&v))
                .map_err(|x| x.to_string()),
            want(&e["relpath"]),
            "{e}"
        );
        assert_eq!(
            ntpath::commonpath(&[a, b])
                .map(|v| u8s(&v))
                .map_err(|x| x.to_string()),
            want(&e["commonpath"]),
            "{e}"
        );
        let (wa, wb, wc): (Vec<u16>, Vec<u16>, Vec<u16>) = (
            s(&e["a"]).encode_utf16().collect(),
            s(&e["b"]).encode_utf16().collect(),
            s(&e["cwd"]).encode_utf16().collect(),
        );
        assert_eq!(
            String::from_utf16(&ntpath::join(&wa, &[&wb])).unwrap(),
            s(&e["join"]),
            "{e}"
        );
        assert_eq!(
            ntpath::relpath(&wa, Some(&wb), &wc)
                .map(|v| String::from_utf16(&v).unwrap())
                .map_err(|x| x.to_string()),
            want(&e["relpath"]),
            "{e}"
        );
    }
}

#[test]
fn text_mode_random() {
    for e in sec("text_io_random") {
        let t = s(&e["in"]);
        assert_eq!(textio::universal_newlines(t), s(&e["newline_none"]), "{e}");
        assert_eq!(
            textio::windows_text_write(t).as_bytes(),
            hex(&e["windows_hex"]),
            "{e}"
        );
    }
}

#[test]
fn split_family_random() {
    for e in sec("str_split_random") {
        let t = s(&e["in"]);
        for (m, field) in [(0, "split0"), (2, "split2"), (3, "split3")] {
            assert_eq!(
                text::split_whitespace(t, Some(m)).collect::<Vec<_>>(),
                strs(&e[field]),
                "{field} {e}"
            );
        }
        for (m, field) in [(Some(0), "rsplit0"), (Some(1), "rsplit1"), (None, "rsplit")] {
            assert_eq!(
                text::rsplit_whitespace(t, m),
                strs(&e[field]),
                "{field} {e}"
            );
        }
        let (h, sep, tail) = text::partition(t, ",");
        assert_eq!(vec![h, sep, tail], strs(&e["partition"]), "{e}");
        assert_eq!(text::strip_chars(t, "#,\\ "), s(&e["strip_chars"]), "{e}");
        assert_eq!(text::lstrip_chars(t, "#,\\ "), s(&e["lstrip_chars"]), "{e}");
        assert_eq!(text::rstrip_chars(t, "#,\\ "), s(&e["rstrip_chars"]), "{e}");
    }
    for e in sec("bytes_split_random") {
        let b = hex(&e["in_hex"]);
        for (m, field) in [
            (None, "rsplit_-1"),
            (Some(0), "rsplit_0"),
            (Some(1), "rsplit_1"),
            (Some(3), "rsplit_3"),
        ] {
            let got: Vec<Vec<u8>> = text::bytes_rsplit_whitespace(&b, m)
                .into_iter()
                .map(<[u8]>::to_vec)
                .collect();
            assert_eq!(got, hexes(&e[field]), "{field} {e}");
        }
    }
}

/// The bytes pre-push parse (D-CRASH-11): `bytes.splitlines()` + `bytes.rsplit(maxsplit=3)` with
/// the unpack error, lazily.
#[test]
fn pre_push_bytes_parse() {
    for e in sec("pre_push_parse_bytes") {
        let b = hex(&e["in_hex"]);
        let want = e["out"].as_array().unwrap();
        let mut n = 0;
        for (got, w) in precommit::pre_push_lines_bytes(&b).zip(want) {
            n += 1;
            match got {
                Ok(fields) => {
                    let got: Vec<Vec<u8>> = fields.iter().map(|f| f.to_vec()).collect();
                    assert_eq!(got, hexes(&w["fields_hex"]), "{e}");
                }
                Err(u) => {
                    assert_eq!(u.to_string(), s(&w["msg"]), "{e}");
                    break;
                }
            }
        }
        assert_eq!(n, want.len(), "{e}");
        // valid UTF-8 input parses the same through the str path
        if let Ok(st) = std::str::from_utf8(&b) {
            let bytes_view: Vec<_> = precommit::pre_push_lines_bytes(&b)
                .map(|r| r.map(|f| f.map(<[u8]>::to_vec)))
                .collect();
            let str_view: Vec<_> = precommit::pre_push_lines(st)
                .map(|r| {
                    r.map(|l| {
                        [l.local_branch, l.local_sha, l.remote_branch, l.remote_sha]
                            .map(|x| x.as_bytes().to_vec())
                    })
                })
                .collect();
            if st.is_ascii() && !st.contains(['\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\x1f']) {
                assert_eq!(bytes_view, str_view, "{e}");
            }
        }
    }
    // the OsStr helpers of the exe_exists composite accept any bytes
    assert!(precommit::exe_exists_pure(
        OsStr::new("/usr/bin/x"),
        OsStr::new("/home/u")
    ));
}
