//! The 59 original sections (15 §A1–§A4) replayed against the frozen API of ARCHITECTURE §1.4.2.
//! Vendored from `docs/research/pycompat-proto/tests/vectors.rs` (WP-00) and re-enabled by WP-11
//! once the vectors were regenerated (ADR-053). Where snp follows CPython 3.13 (`isprintable`,
//! ADR-020) the 3.13 view of the section is used.

use std::ffi::OsStr;

use snp_pycompat::{chars, codec, int, ntpath, posixpath, precommit, repr, shlex, text, textio};

use crate::common::{
    V, blines, by, hex, hexes, in_ranges, lines, merge_conflicts, outcome, pres, py_str, ranges,
    repr_of, s, sec, skips, strs, u8s, u32_of, untag, usize_of, view313, want,
};
use crate::support::json::Value;

#[test]
fn char_sets() {
    let sets = &V["sets"];
    let isspace = ranges(&sets["str_isspace"]);
    let rs = ranges(&sets["re_str_backslash_s"]);
    let lb = ranges(&sets["str_splitlines_boundary"]);
    // repr/isprintable follow UCD 15.1 (CPython 3.13, ADR-020)
    let sets313 = view313("sets");
    let np = ranges(&sets313["str_isprintable_false"]);
    for cp in 0..=0x10_ffff_u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        assert_eq!(
            chars::is_space(c),
            in_ranges(cp, &isspace),
            "isspace U+{cp:04X}"
        );
        assert_eq!(chars::is_space(c), in_ranges(cp, &rs), "re \\s U+{cp:04X}");
        assert_eq!(
            chars::is_linebreak(c),
            in_ranges(cp, &lb),
            "linebreak U+{cp:04X}"
        );
        assert_eq!(
            !chars::is_printable(c),
            in_ranges(cp, &np),
            "isprintable U+{cp:04X}"
        );
    }
    let bs: Vec<u8> = sets["bytes_isspace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| u8::try_from(x.as_u64().unwrap()).unwrap())
        .collect();
    let bre: Vec<u8> = sets["re_bytes_backslash_s"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| u8::try_from(x.as_u64().unwrap()).unwrap())
        .collect();
    let blb: Vec<u8> = sets["bytes_splitlines_boundary"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| u8::try_from(x.as_u64().unwrap()).unwrap())
        .collect();
    for b in 0..=255_u8 {
        assert_eq!(
            chars::is_bytes_space(b),
            bs.contains(&b),
            "bytes isspace {b:#x}"
        );
        assert_eq!(chars::is_bytes_space(b), bre.contains(&b), "re b\\s {b:#x}");
        let input = [b'a', b, b'b'];
        assert_eq!(
            text::bytes_splitlines(&input).count() == 2,
            blb.contains(&b),
            "bytes splitlines boundary {b:#x}"
        );
    }
    // the Rust std set really differs (15 §A3): U+001C..U+001F only
    let minus = ranges(&sets["str_isspace_minus_rust_white_space"]);
    assert_eq!(minus, vec![(0x1c, 0x1f)]);
    for z in V["decimal_zeros"].as_array().unwrap().iter().map(u32_of) {
        for d in 0..10 {
            assert_eq!(
                chars::decimal_value(char::from_u32(z + d).unwrap()),
                Some(d)
            );
        }
    }
    let zeros: Vec<u32> = V["decimal_zeros"]
        .as_array()
        .unwrap()
        .iter()
        .map(u32_of)
        .collect();
    for cp in 0..=0x10_ffff_u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let is_digit = zeros.iter().any(|&z| (z..z + 10).contains(&cp));
        assert_eq!(
            chars::decimal_value(c).is_some(),
            is_digit,
            "decimal U+{cp:04X}"
        );
    }
}

#[test]
fn splitlines_split_strip() {
    for e in sec("str_splitlines") {
        assert_eq!(
            lines(s(&e["in"]), e["keepends"].as_bool().unwrap()),
            strs(&e["out"]),
            "{e}"
        );
    }
    for e in sec("bytes_splitlines") {
        assert_eq!(
            blines(&hex(&e["in_hex"]), e["keepends"].as_bool().unwrap()),
            hexes(&e["out_hex"]),
            "{e}"
        );
    }
    for e in sec("str_split_none") {
        assert_eq!(
            text::split_whitespace(s(&e["in"]), None).collect::<Vec<_>>(),
            strs(&e["out"]),
            "{e}"
        );
    }
    for e in sec("str_split_none_maxsplit") {
        let m = usize_of(&e["maxsplit"]);
        assert_eq!(
            text::split_whitespace(s(&e["in"]), Some(m)).collect::<Vec<_>>(),
            strs(&e["out"]),
            "{e}"
        );
    }
    for e in sec("str_rsplit_none_maxsplit") {
        let m = e["maxsplit"].as_i64().unwrap();
        let m = usize::try_from(m).ok();
        assert_eq!(
            text::rsplit_whitespace(s(&e["in"]), m),
            strs(&e["out"]),
            "{e}"
        );
        if m == Some(3) {
            // the pre-push unpack sees the same fields
            let want = strs(&e["out"]);
            match text::rsplit4(s(&e["in"])) {
                Ok(f) => assert_eq!(f.to_vec(), want, "{e}"),
                Err(u) => assert_eq!(u.got, want.len(), "{e}"),
            }
        }
    }
    for e in sec("str_split_sep") {
        let got: Vec<&str> = s(&e["in"]).split(s(&e["sep"])).collect();
        assert_eq!(got, strs(&e["out"]), "{e}");
    }
    for e in sec("str_strip_none") {
        let i = s(&e["in"]);
        assert_eq!(text::strip(i), s(&e["strip"]));
        assert_eq!(text::lstrip(i), s(&e["lstrip"]));
        assert_eq!(text::rstrip(i), s(&e["rstrip"]));
    }
    for e in sec("str_strip_chars") {
        let (i, c) = (s(&e["in"]), s(&e["chars"]));
        assert_eq!(text::strip_chars(i, c), s(&e["strip"]));
        assert_eq!(text::lstrip_chars(i, c), s(&e["lstrip"]));
        assert_eq!(text::rstrip_chars(i, c), s(&e["rstrip"]));
    }
    for e in sec("bytes_strip_none") {
        let b = hex(&e["in_hex"]);
        assert_eq!(text::bytes_strip(&b), hex(&e["strip_hex"]));
        assert_eq!(text::bytes_lstrip(&b), hex(&e["lstrip_hex"]));
        assert_eq!(text::bytes_rstrip(&b), hex(&e["rstrip_hex"]));
    }
    for e in sec("bytes_strip_chars") {
        let (b, c) = (hex(&e["in_hex"]), hex(&e["chars_hex"]));
        assert_eq!(text::bytes_strip_set(&b, &c), hex(&e["strip_hex"]));
        assert_eq!(text::bytes_lstrip_set(&b, &c), hex(&e["lstrip_hex"]));
        assert_eq!(text::bytes_rstrip_set(&b, &c), hex(&e["rstrip_hex"]));
    }
}

#[test]
fn precommit_composites() {
    for e in sec("zsplit") {
        let got: Vec<&[u8]> = precommit::zsplit(s(&e["in"]).as_bytes()).collect();
        let want: Vec<&[u8]> = strs(&e["out"]).into_iter().map(str::as_bytes).collect();
        assert_eq!(got, want, "{e}");
    }
    for e in sec("skip_env") {
        assert_eq!(skips(s(&e["in"])), strs(&e["out_sorted_set"]), "{e}");
    }
    for e in sec("pre_push_parse") {
        let b = hex(&e["in_hex"]);
        let out = &e["out"];
        match codec::decode_utf8(&b) {
            Err(de) => {
                assert_eq!(de.to_string(), s(&out["decode"]["msg"]), "{e}");
                assert_eq!(de.py_type(), s(&out["decode"]["error"]), "{e}");
            }
            Ok(stdin) => {
                let want = out["lines"].as_array().unwrap();
                let mut n = 0;
                for (got, w) in precommit::pre_push_lines(stdin).zip(want) {
                    n += 1;
                    match got {
                        Ok(l) => assert_eq!(
                            vec![l.local_branch, l.local_sha, l.remote_branch, l.remote_sha],
                            strs(&w["fields"]),
                            "{e}"
                        ),
                        Err(u) => {
                            assert_eq!(u.to_string(), s(&w["msg"]), "{e}");
                            assert_eq!(u.py_type(), s(&w["error"]), "{e}");
                            break;
                        }
                    }
                }
                assert_eq!(n, want.len(), "{e}");
            }
        }
    }
    for e in sec("merge_msg_conflicts") {
        let b = hex(&e["in_hex"]);
        match (merge_conflicts(&b), outcome(&e["out"])) {
            (Ok(got), Ok(w)) => assert_eq!(got, strs(w), "{e}"),
            (Err(g), Err((_, m))) => assert_eq!(g, m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
    for e in sec("rev_line_re") {
        let got = precommit::rev_line_match(s(&e["in"])).map(|g| g.to_vec());
        let want = e["groups"].as_array().map(|_| strs(&e["groups"]));
        assert_eq!(got, want, "{e}");
    }
    for e in sec("rev_line_rewrite") {
        let got =
            precommit::rev_line_rewrite(s(&e["line"]), s(&e["new_rev"]), e["frozen"].as_str())
                .unwrap();
        assert_eq!(got, s(&e["out"]), "{e}");
    }
    for e in sec("autoupdate_original_lines") {
        let ls = lines(s(&e["in"]), true);
        assert_eq!(ls, strs(&e["out"]["lines"]));
        let idxs: Vec<u64> = ls
            .iter()
            .enumerate()
            .filter(|(_, l)| precommit::rev_line_match(l).is_some())
            .map(|(i, _)| u64::try_from(i).unwrap())
            .collect();
        let want: Vec<u64> = e["out"]["idxs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap())
            .collect();
        assert_eq!(idxs, want, "{e}");
    }
    for e in sec("migrate_header_split") {
        let (h, r) = precommit::migrate_header_split(s(&e["in"]));
        assert_eq!(
            (h, r),
            (s(&e["out"]["header"]), s(&e["out"]["rest"])),
            "{e}"
        );
        assert_eq!(
            textio::textwrap_indent(r, "    "),
            s(&e["out"]["indented_rest"]),
            "{e}"
        );
    }
    for e in sec("textwrap_indent") {
        assert_eq!(
            textio::textwrap_indent(s(&e["in"]), s(&e["prefix"])),
            s(&e["out"]),
            "{e}"
        );
    }
    let host = if cfg!(windows) { "nt" } else { "posix" };
    for e in sec("exe_exists_pure") {
        if s(&e["flavor"]) == host {
            let got =
                precommit::exe_exists_pure(OsStr::new(s(&e["found"])), OsStr::new(s(&e["home"])));
            assert_eq!(got, e["out"].as_bool().unwrap(), "{e}");
        }
    }
}

/// `REV_LINE_RE` (15 §A4.6): the 32 hand-picked lines and the 600 seeded random lines.
#[test]
fn rev_line_match_agrees_with_python() {
    for name in ["rev_line_re", "random_rev_lines"] {
        for e in sec(name) {
            let want = e["groups"].as_array().map(|_| strs(&e["groups"]));
            let got = precommit::rev_line_match(s(&e["in"])).map(|g| g.to_vec());
            assert_eq!(got, want, "{e}");
        }
    }
}

#[test]
fn text_io_and_codecs() {
    for e in sec("text_read") {
        let b = hex(&e["in_hex"]);
        let st = std::str::from_utf8(&b).unwrap();
        assert_eq!(textio::universal_newlines(st), s(&e["newline_none"]), "{e}");
        assert_eq!(st, s(&e["newline_empty"]), "{e}");
    }
    for e in sec("text_write") {
        let i = s(&e["in"]);
        assert_eq!(
            textio::windows_text_write(i).as_bytes(),
            hex(&e["windows_hex"]),
            "{e}"
        );
        assert_eq!(i.as_bytes(), hex(&e["posix_hex"]), "{e}");
        assert_eq!(i.as_bytes(), hex(&e["newline_empty_hex"]), "{e}");
    }
    for e in sec("utf8_decode_strict") {
        let b = hex(&e["in_hex"]);
        match (codec::decode_utf8(&b), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, s(w)),
            (Err(g), Err((t, m))) => {
                assert_eq!(t, g.py_type());
                assert_eq!(g.to_string(), m, "{e}");
            }
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
    for e in sec("utf8_decode_backslashreplace") {
        assert_eq!(
            codec::decode_utf8_backslashreplace(&hex(&e["in_hex"])),
            s(&e["out"]),
            "{e}"
        );
    }
}

#[test]
fn reprs() {
    for e in sec("repr") {
        assert_eq!(repr_of(&e["in"]), s(&e["out"]), "{e}");
    }
    for e in sec("fstring_eq") {
        assert_eq!(
            repr::fstring_eq(s(&e["name"]), &py_str(&e["in"]).text()),
            s(&e["out"])
        );
    }
    for e in sec("oserror_str") {
        let a = e["args"].as_array().unwrap();
        if a.len() < 2 {
            continue; // OSError(msg) / OSError() are plain str(args)
        }
        let errno = i32::try_from(a[0].as_i64().unwrap()).unwrap();
        let got = repr::oserror_str(
            errno,
            s(&a[1]),
            a.get(2).and_then(Value::as_str).map(OsStr::new),
            a.get(4).and_then(Value::as_str).map(OsStr::new),
        );
        assert_eq!(got, s(&e["out"]), "{e}");
    }
    #[cfg(target_os = "linux")]
    for (n, msg) in V["errno_strerror_glibc"].as_object().unwrap() {
        // io::Error's text minus " (os error N)" is glibc's strerror, like os.strerror
        let code: i32 = n.parse().unwrap();
        let got = repr::oserror_from_io(&std::io::Error::from_raw_os_error(code), None, None);
        assert_eq!(got, format!("[Errno {code}] {}", s(msg)), "errno {n}");
    }
    for e in sec("stdlib_calledprocesserror_str") {
        let cmd = &e["cmd"];
        let cmd_str = if s(&cmd["t"]) == "str" {
            py_str(cmd).text()
        } else {
            repr::repr(&untag(cmd))
        };
        let rc = e["returncode"].as_i64().unwrap();
        let got = repr::stdlib_called_process_error_str(rc, &cmd_str);
        if rc < 0 && !cfg!(target_os = "linux") {
            continue; // `signal.Signals` is per platform; the vectors come from Linux
        }
        assert_eq!(got, s(&e["out"]), "{e}");
    }
    for e in sec("keyerror_str") {
        assert_eq!(repr::repr_str(s(&e["in"])), s(&e["out"]));
    }
    for e in sec("argparse_invalid_choice") {
        let choices: Vec<String> = strs(&e["choices"])
            .iter()
            .map(|c| repr::repr_str(c))
            .collect();
        let got = format!(
            "invalid choice: {} (choose from {})",
            repr::repr_str(s(&e["value"])),
            choices.join(", ")
        );
        assert_eq!(got, s(&e["out"]));
    }
    for e in sec("parse_version") {
        match (int::parse_version(s(&e["in"])), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => {
                let w: Vec<i128> = w
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_i128().unwrap())
                    .collect();
                assert_eq!(g, w, "{e}");
            }
            (Err(g), Err((t, m))) => {
                assert_eq!(g.to_string(), m, "{e}");
                assert_eq!(g.py_type(), t, "{e}");
            }
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

#[test]
fn shlex_vectors() {
    for e in sec("shlex_quote") {
        assert_eq!(shlex::quote(s(&e["in"])), s(&e["out"]), "{e}");
        assert_eq!(
            &*shlex::quote_bytes(s(&e["in"]).as_bytes()),
            s(&e["out"]).as_bytes(),
            "{e}"
        );
    }
    for e in sec("shlex_join") {
        assert_eq!(shlex::join(strs(&e["in"])), s(&e["out"]), "{e}");
    }
    for e in sec("shlex_split") {
        match (shlex::split(s(&e["in"])), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, strs(w), "{e}"),
            (Err(g), Err((t, m))) => {
                assert_eq!(g.to_string(), m, "{e}");
                assert_eq!(g.py_type(), t, "{e}");
            }
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

#[test]
fn posixpath_vectors() {
    for e in sec("posixpath_normpath") {
        assert_eq!(
            u8s(&posixpath::normpath(by(s(&e["in"])))),
            s(&e["out"]),
            "{e}"
        );
    }
    for e in sec("posixpath_abspath") {
        assert_eq!(
            Ok(u8s(&posixpath::abspath(by(s(&e["in"])), by(s(&e["cwd"]))))),
            want(&e["out"]),
            "{e}"
        );
    }
    for e in sec("posixpath_join") {
        let parts = strs(&e["in"]);
        let rest: Vec<&[u8]> = parts[1..].iter().map(|x| x.as_bytes()).collect();
        assert_eq!(
            u8s(&posixpath::join(by(parts[0]), &rest)),
            s(&e["out"]),
            "{e}"
        );
    }
    for e in sec("posixpath_relpath") {
        let got = posixpath::relpath(
            by(s(&e["path"])),
            e["start"].as_str().map(str::as_bytes),
            by(s(&e["cwd"])),
        );
        assert_eq!(pres(got), want(&e["out"]), "{e}");
    }
    for e in sec("posixpath_commonpath") {
        let ps: Vec<&[u8]> = strs(&e["in"]).into_iter().map(str::as_bytes).collect();
        assert_eq!(pres(posixpath::commonpath(&ps)), want(&e["out"]), "{e}");
    }
    for e in sec("posixpath_parts") {
        let p = by(s(&e["in"]));
        assert_eq!(u8s(posixpath::dirname(p)), s(&e["dirname"]), "{e}");
        assert_eq!(u8s(posixpath::basename(p)), s(&e["basename"]), "{e}");
        let (head, tail) = posixpath::split(p);
        assert_eq!(vec![u8s(head), u8s(tail)], strs(&e["split"]), "{e}");
        let (root, ext) = posixpath::splitext(p);
        assert_eq!(vec![u8s(root), u8s(ext)], strs(&e["splitext"]), "{e}");
        assert_eq!(posixpath::isabs(p), e["isabs"].as_bool().unwrap());
    }
    let none = || None;
    let named = |_: &[u8]| None;
    for e in sec("posixpath_expanduser") {
        let home = e["env"]
            .get("HOME")
            .and_then(Value::as_str)
            .map(str::as_bytes);
        let got = posixpath::expanduser(by(s(&e["in"])), home, &none, &named);
        assert_eq!(Ok(u8s(&got)), want(&e["out"]), "{e}");
    }
    for e in sec("random_posixpath") {
        let (a, bb) = (s(&e["a"]), s(&e["b"]));
        let cwd = b"/home/u/repo/sub";
        assert_eq!(u8s(&posixpath::normpath(by(a))), s(&e["normpath"]), "{e}");
        assert_eq!(
            u8s(&posixpath::join(by(a), &[by(bb)])),
            s(&e["join"]),
            "{e}"
        );
        let (head, tail) = posixpath::split(by(a));
        assert_eq!(vec![u8s(head), u8s(tail)], strs(&e["split"]), "{e}");
        let (root, ext) = posixpath::splitext(by(a));
        assert_eq!(vec![u8s(root), u8s(ext)], strs(&e["splitext"]), "{e}");
        assert_eq!(
            Ok(u8s(&posixpath::abspath(by(a), cwd))),
            want(&e["abspath"]),
            "{e}"
        );
        let start = (!bb.is_empty()).then_some(by(bb));
        assert_eq!(
            pres(posixpath::relpath(by(a), start, cwd)),
            want(&e["relpath"]),
            "{e}"
        );
        assert_eq!(
            pres(posixpath::commonpath(&[by(a), by(bb)])),
            want(&e["commonpath"]),
            "{e}"
        );
    }
}

/// The realpath fixture of 15 §A4.9 (V-RP), recreated under a temp dir with `$ROOT` substitution
/// and the cwd passed explicitly (no `chdir`).
#[cfg(unix)]
#[test]
fn posixpath_realpath_fixture() {
    use std::os::unix::ffi::OsStrExt;
    let fx = &V["posixpath_realpath_fixture"];
    let tmp = std::env::temp_dir().join(format!("pycompat-realpath-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).unwrap_or_default();
    std::fs::create_dir_all(&tmp).unwrap();
    #[expect(
        clippy::disallowed_methods,
        reason = "test fixture root: the physical temp dir"
    )]
    let root = std::fs::canonicalize(&tmp).unwrap();
    let rootb = root.as_os_str().as_bytes().to_vec();
    let rs = String::from_utf8(rootb.clone()).unwrap();
    for d in strs(&fx["dirs"]) {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for f in strs(&fx["files"]) {
        std::fs::write(root.join(f), b"x").unwrap();
    }
    for l in fx["symlinks"].as_array().unwrap() {
        std::os::unix::fs::symlink(s(&l[1]).replace("$ROOT", &rs), root.join(s(&l[0]))).unwrap();
    }
    let cases = fx["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 29);
    for c in cases {
        let input = s(&c["in"]).replace("$ROOT", &rs);
        let got = posixpath::realpath(input.as_bytes(), &rootb).unwrap();
        let got = String::from_utf8(got).unwrap().replace(&rs, "$ROOT");
        assert_eq!(got, s(&c["out"]), "{c}");
        // the facade gives the same answer
        let facade =
            snp_pycompat::ospath::realpath(OsStr::new(&input), std::path::Path::new(&rs)).unwrap();
        assert_eq!(
            facade.to_str().unwrap().replace(&rs, "$ROOT"),
            s(&c["out"]),
            "{c}"
        );
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn ntpath_vectors() {
    let nt_err =
        |r: Result<Vec<u8>, snp_pycompat::PathError>| r.map(|v| u8s(&v)).map_err(|x| x.to_string());
    for e in sec("ntpath_normpath") {
        assert_eq!(u8s(&ntpath::normpath(by(s(&e["in"])))), s(&e["out"]), "{e}");
    }
    for e in sec("ntpath_abspath_fallback") {
        assert_eq!(
            Ok(u8s(&ntpath::abspath_lexical(
                by(s(&e["in"])),
                by(s(&e["cwd"]))
            ))),
            want(&e["out"]),
            "{e}"
        );
    }
    for e in sec("ntpath_join") {
        let parts = strs(&e["in"]);
        let rest: Vec<&[u8]> = parts[1..].iter().map(|x| x.as_bytes()).collect();
        assert_eq!(u8s(&ntpath::join(by(parts[0]), &rest)), s(&e["out"]), "{e}");
    }
    for e in sec("ntpath_relpath") {
        let got = ntpath::relpath(
            by(s(&e["path"])),
            e["start"].as_str().map(str::as_bytes),
            by(s(&e["cwd"])),
        );
        assert_eq!(nt_err(got), want(&e["out"]), "{e}");
    }
    for e in sec("ntpath_commonpath") {
        let ps: Vec<&[u8]> = strs(&e["in"]).into_iter().map(str::as_bytes).collect();
        assert_eq!(nt_err(ntpath::commonpath(&ps)), want(&e["out"]), "{e}");
    }
    for e in sec("ntpath_parts") {
        let p = by(s(&e["in"]));
        assert_eq!(u8s(&ntpath::dirname(p)), s(&e["dirname"]), "{e}");
        assert_eq!(u8s(ntpath::basename(p)), s(&e["basename"]), "{e}");
        let (head, tail) = ntpath::split(p);
        assert_eq!(vec![u8s(&head), u8s(tail)], strs(&e["split"]), "{e}");
        let (root, ext) = ntpath::splitext(p);
        assert_eq!(vec![u8s(root), u8s(ext)], strs(&e["splitext"]), "{e}");
        let (d, rest) = ntpath::splitdrive(p);
        assert_eq!(vec![u8s(d), u8s(rest)], strs(&e["splitdrive"]), "{e}");
        assert_eq!(ntpath::isabs_312(p), e["isabs"].as_bool().unwrap(), "{e}");
        assert_eq!(u8s(&ntpath::normcase(p)), s(&e["normcase"]), "{e}");
    }
    for e in sec("ntpath_expanduser") {
        let env = e["env"].as_object().unwrap().clone();
        let lookup = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .and_then(|(_, v)| v.as_str())
                .map(|v| v.as_bytes().to_vec())
        };
        let got = ntpath::expanduser(by(s(&e["in"])), &lookup);
        assert_eq!(Ok(u8s(&got)), want(&e["out"]), "{e}");
    }
    for e in sec("random_ntpath") {
        let (a, bb) = (s(&e["a"]), s(&e["b"]));
        let cwd = b"C:\\Users\\u\\repo\\sub";
        assert_eq!(u8s(&ntpath::normpath(by(a))), s(&e["normpath"]), "{e}");
        assert_eq!(u8s(&ntpath::join(by(a), &[by(bb)])), s(&e["join"]), "{e}");
        let (head, tail) = ntpath::split(by(a));
        assert_eq!(vec![u8s(&head), u8s(tail)], strs(&e["split"]), "{e}");
        let (root, ext) = ntpath::splitext(by(a));
        assert_eq!(vec![u8s(root), u8s(ext)], strs(&e["splitext"]), "{e}");
        let (d, rest) = ntpath::splitdrive(by(a));
        assert_eq!(vec![u8s(d), u8s(rest)], strs(&e["splitdrive"]), "{e}");
        assert_eq!(
            ntpath::isabs_312(by(a)),
            e["isabs"].as_bool().unwrap(),
            "{e}"
        );
        assert_eq!(u8s(&ntpath::normcase(by(a))), s(&e["normcase"]), "{e}");
        assert_eq!(
            Ok(u8s(&ntpath::abspath_lexical(by(a), cwd))),
            want(&e["abspath"]),
            "{e}"
        );
        let start = (!bb.is_empty()).then_some(by(bb));
        assert_eq!(
            nt_err(ntpath::relpath(by(a), start, cwd)),
            want(&e["relpath"]),
            "{e}"
        );
        assert_eq!(
            nt_err(ntpath::commonpath(&[by(a), by(bb)])),
            want(&e["commonpath"]),
            "{e}"
        );
    }
}

/// `main._adjust_args_and_chdir` (V-ADJ): `abspath` in the invocation cwd, then `relpath` from the
/// toplevel (both flavours, lexically).
#[test]
fn adjust_args_and_chdir() {
    for e in sec("adjust_args_and_chdir") {
        let (cwd, top, arg) = (s(&e["cwd"]), s(&e["toplevel"]), s(&e["arg"]));
        let make_abs = s(&e["kind"]) == "files" || e["exists"].as_bool().unwrap();
        let got: Result<String, String> = if s(&e["flavor"]) == "posix" {
            let a = if make_abs {
                posixpath::abspath(arg.as_bytes(), cwd.as_bytes())
            } else {
                arg.as_bytes().to_vec()
            };
            pres(posixpath::relpath(&a, None, top.as_bytes()))
        } else {
            let a = if make_abs {
                ntpath::abspath_lexical(arg.as_bytes(), cwd.as_bytes())
            } else {
                arg.as_bytes().to_vec()
            };
            ntpath::relpath(&a, None, top.as_bytes())
                .map(|v| u8s(&v))
                .map_err(|x| x.to_string())
        };
        assert_eq!(got, want(&e["out"]), "{e}");
        if let (Ok(g), Some(cl)) = (&got, e["out"].get("classifier")) {
            assert_eq!(g.replace('\\', "/"), s(cl));
        }
    }
}

/// `parse_shebang.find_executable` (POSIX flavour) restated over the shims (normpath, join); the
/// real lookup lives in snp-proc. Relative candidates are checked against `cwd` (snp never
/// chdirs); the executable bit stands in for `access(X_OK)`.
#[cfg(unix)]
fn find_executable(
    exe: &str,
    path: Option<&str>,
    pathext: Option<&str>,
    cwd: &std::path::Path,
) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let exe = u8s(&posixpath::normpath(exe.as_bytes()));
    if exe.contains('/') {
        return Some(exe);
    }
    let mut names: Vec<String> = Vec::new();
    if let Some(exts) = pathext {
        names.extend(exts.split(':').map(|e| format!("{exe}{e}")));
    }
    names.push(exe);
    for dir in path.unwrap_or("").split(':') {
        for name in &names {
            let joined = u8s(&posixpath::join(dir.as_bytes(), &[name.as_bytes()]));
            let meta = std::fs::metadata(cwd.join(&joined));
            if meta.is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
                return Some(joined);
            }
        }
    }
    None
}

#[cfg(unix)]
#[test]
fn find_executable_fixture() {
    use std::os::unix::fs::PermissionsExt;
    let fx = &V["find_executable_posix_fixture"];
    let tmp = std::env::temp_dir().join(format!("pycompat-fe-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).unwrap_or_default();
    std::fs::create_dir_all(&tmp).unwrap();
    #[expect(
        clippy::disallowed_methods,
        reason = "test fixture root: the physical temp dir"
    )]
    let root = std::fs::canonicalize(&tmp).unwrap();
    let rs = root.to_str().unwrap().to_owned();
    for d in strs(&fx["dirs"]) {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for fm in fx["files_mode"].as_array().unwrap() {
        let p = root.join(s(&fm[0]));
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        let mode = u32::from_str_radix(s(&fm[1]), 8).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    for c in fx["cases"].as_array().unwrap() {
        let env = &c["env"];
        let path = env
            .get("PATH")
            .and_then(Value::as_str)
            .map(|v| v.replace("$ROOT", &rs));
        let pathext = env
            .get("PATHEXT")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let got = find_executable(s(&c["exe"]), path.as_deref(), pathext.as_deref(), &root)
            .map(|p| p.replace(&rs, "$ROOT"));
        assert_eq!(got.as_deref(), c["out"].as_str(), "{c}");
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// The seeded random corpora (`random.Random(20261008)`, 400 cases each).
#[test]
fn random_corpora() {
    for e in sec("random_str_ops") {
        let t = s(&e["in"]);
        assert_eq!(lines(t, false), strs(&e["splitlines"]), "{e}");
        assert_eq!(lines(t, true), strs(&e["splitlines_keepends"]), "{e}");
        let mut back: Vec<&str> = text::splitlines_keepends(t).rev().collect();
        back.reverse();
        assert_eq!(back, strs(&e["splitlines_keepends"]), "{e}");
        assert_eq!(
            text::split_whitespace(t, None).collect::<Vec<_>>(),
            strs(&e["split"]),
            "{e}"
        );
        assert_eq!(
            text::split_whitespace(t, Some(1)).collect::<Vec<_>>(),
            strs(&e["split_max1"]),
            "{e}"
        );
        assert_eq!(
            text::rsplit_whitespace(t, Some(3)),
            strs(&e["rsplit_max3"]),
            "{e}"
        );
        assert_eq!(text::strip(t), s(&e["strip"]), "{e}");
        assert_eq!(text::lstrip(t), s(&e["lstrip"]), "{e}");
        assert_eq!(text::rstrip(t), s(&e["rstrip"]), "{e}");
        assert_eq!(repr::repr_str(t), s(&e["repr"]), "{e}");
        assert_eq!(shlex::quote(t), s(&e["shlex_quote"]), "{e}");
        match (shlex::split(t), outcome(&e["shlex_split"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, strs(w), "{e}"),
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
        assert_eq!(
            textio::textwrap_indent(t, "  "),
            s(&e["textwrap_indent"]),
            "{e}"
        );
        assert_eq!(skips(t), strs(&e["skips"]), "{e}");
    }
    for e in sec("random_bytes_ops") {
        let b = hex(&e["in_hex"]);
        assert_eq!(blines(&b, false), hexes(&e["splitlines_hex"]), "{e}");
        assert_eq!(
            blines(&b, true),
            hexes(&e["splitlines_keepends_hex"]),
            "{e}"
        );
        assert_eq!(text::bytes_strip(&b), hex(&e["strip_hex"]), "{e}");
        assert_eq!(
            text::bytes_rstrip_set(&b, b"\r\n"),
            hex(&e["rstrip_crlf_hex"]),
            "{e}"
        );
        match (codec::decode_utf8(&b), outcome(&e["decode"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, s(w)),
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
        assert_eq!(
            codec::decode_utf8_backslashreplace(&b),
            s(&e["decode_backslashreplace"]),
            "{e}"
        );
        match (merge_conflicts(&b), outcome(&e["merge_conflicts"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, strs(w), "{e}"),
            (Err(g), Err((_, m))) => assert_eq!(g, m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

/// The documented Rust-std pitfalls of 15 §A8 (no vector file needed).
#[test]
fn rust_std_pitfalls_are_real() {
    assert!(precommit::rev_line_match("\x1f rev: v1\n").is_some());
    // str::lines() does not split on \r, \x0b, \x0c, \x1c-\x1e, \x85, U+2028/9
    assert_eq!("a\rb\x0bc".lines().count(), 1);
    assert_eq!(text::splitlines("a\rb\x0bc").count(), 3);
    // str::split_whitespace / trim use White_Space (no \x1c-\x1f)
    assert_eq!("a\x1fb".split_whitespace().count(), 1);
    assert_eq!(text::split_whitespace("a\x1fb", None).count(), 2);
    // <[u8]>::trim_ascii omits \x0b
    assert_eq!(b"a\x0b".trim_ascii(), b"a\x0b");
    assert_eq!(text::bytes_strip(b"a\x0b"), b"a");
    // f64 Display is not Python repr
    assert_eq!(format!("{}", 1e16_f64), "10000000000000000");
    assert_eq!(repr::repr_float(1e16), "1e+16");
    // Path::file_name / extension are not basename / splitext
    assert_eq!(
        std::path::Path::new("a/b/").file_name(),
        Some(OsStr::new("b"))
    );
    assert_eq!(posixpath::basename(b"a/b/"), b"");
    assert_eq!(
        std::path::Path::new("...md").extension(),
        Some(OsStr::new("md"))
    );
    assert_eq!(posixpath::splitext(b"...md"), (&b"...md"[..], &b""[..]));
}
