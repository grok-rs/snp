//! Replays `research/vectors/pycompat-vectors.cpython312.json`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::LazyLock;

use serde_json::Value;
use snp_pycompat_proto::{codec, int, ntpath, posixpath, precommit, repr, shlex, text, textio, unicode};

static V: LazyLock<Value> = LazyLock::new(|| {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vectors/pycompat-vectors.cpython312.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
});

fn sec(name: &str) -> &'static Vec<Value> {
    V[name].as_array().unwrap_or_else(|| panic!("missing section {name}"))
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}
fn strs(v: &Value) -> Vec<&str> {
    v.as_array().unwrap().iter().map(s).collect()
}
fn hex(v: &Value) -> Vec<u8> {
    let h = s(v);
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}
fn hexes(v: &Value) -> Vec<Vec<u8>> {
    v.as_array().unwrap().iter().map(hex).collect()
}
fn ranges(v: &Value) -> Vec<(u32, u32)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| (r[0].as_u64().unwrap() as u32, r[1].as_u64().unwrap() as u32))
        .collect()
}
fn in_ranges(cp: u32, rs: &[(u32, u32)]) -> bool {
    rs.iter().any(|&(a, b)| a <= cp && cp <= b)
}
/// Python `{'ok': x}` / `{'error': T, 'msg': m}` → Result<&Value, (T, m)>
fn outcome(v: &Value) -> Result<&Value, (String, String)> {
    if let Some(ok) = v.get("ok") {
        Ok(ok)
    } else {
        Err((s(&v["error"]).to_owned(), s(&v["msg"]).to_owned()))
    }
}
fn py_str(t: &Value) -> Result<String, Vec<u8>> {
    // Ok(str) or Err(surrogateescape bytes)
    if let Some(v) = t.get("v") {
        return Ok(s(v).to_owned());
    }
    let cps: Vec<u32> = t["v_cps"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
    let mut b = Vec::new();
    for cp in cps {
        if (0xdc80..=0xdcff).contains(&cp) {
            b.push((cp - 0xdc00) as u8);
        } else {
            let mut buf = [0u8; 4];
            b.extend_from_slice(char::from_u32(cp).unwrap().encode_utf8(&mut buf).as_bytes());
        }
    }
    Err(b)
}
fn untag(t: &Value) -> repr::PyValue {
    use repr::PyValue as P;
    match s(&t["t"]) {
        "none" => P::None,
        "bool" => P::Bool(t["v"].as_bool().unwrap()),
        // 10**20 exceeds u64; serde_json (no arbitrary_precision) stores it as an exact f64
        "int" => P::Int(t["v"].as_i64().map_or_else(|| t["v"].as_f64().unwrap() as i128, i128::from)),
        "float" => P::Float(s(&t["v"]).parse().unwrap()),
        "str" => P::Str(py_str(t).unwrap()),
        "list" => P::List(t["v"].as_array().unwrap().iter().map(untag).collect()),
        "tuple" => P::Tuple(t["v"].as_array().unwrap().iter().map(untag).collect()),
        "dict" => P::Dict(t["v"].as_array().unwrap().iter().map(|kv| (untag(&kv[0]), untag(&kv[1]))).collect()),
        other => panic!("tag {other}"),
    }
}

#[test]
fn char_sets() {
    let sets = &V["sets"];
    let isspace = ranges(&sets["str_isspace"]);
    let rs = ranges(&sets["re_str_backslash_s"]);
    let lb = ranges(&sets["str_splitlines_boundary"]);
    let np = ranges(&sets["str_isprintable_false"]);
    for cp in 0..=0x10ffffu32 {
        let Some(c) = char::from_u32(cp) else { continue };
        assert_eq!(unicode::is_space(c), in_ranges(cp, &isspace), "isspace U+{cp:04X}");
        assert_eq!(unicode::is_space(c), in_ranges(cp, &rs), "re \\s U+{cp:04X}");
        assert_eq!(unicode::is_linebreak(c), in_ranges(cp, &lb), "linebreak U+{cp:04X}");
        assert_eq!(!unicode::is_printable(c), in_ranges(cp, &np), "isprintable U+{cp:04X}");
    }
    let bs: Vec<u8> = sets["bytes_isspace"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    for b in 0..=255u8 {
        assert_eq!(unicode::is_ascii_space(b), bs.contains(&b), "bytes isspace {b:#x}");
    }
    // the documented Rust pitfalls
    assert!(!'\x1f'.is_whitespace() && unicode::is_space('\x1f'));
    assert!(!b'\x0b'.is_ascii_whitespace() && unicode::is_ascii_space(b'\x0b'));
    let zeros: Vec<u32> = V["decimal_zeros"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
    for z in zeros {
        for d in 0..10 {
            assert_eq!(unicode::decimal_value(char::from_u32(z + d).unwrap()), Some(d));
        }
    }
}

#[test]
fn splitlines_split_strip() {
    for e in sec("str_splitlines") {
        assert_eq!(text::splitlines(s(&e["in"]), e["keepends"].as_bool().unwrap()), strs(&e["out"]), "{e}");
    }
    for e in sec("bytes_splitlines") {
        let b = hex(&e["in_hex"]);
        let got: Vec<Vec<u8>> =
            text::bytes_splitlines(&b, e["keepends"].as_bool().unwrap()).into_iter().map(<[u8]>::to_vec).collect();
        assert_eq!(got, hexes(&e["out_hex"]), "{e}");
    }
    for e in sec("str_split_none") {
        assert_eq!(text::split_whitespace(s(&e["in"]), None), strs(&e["out"]), "{e}");
    }
    for e in sec("str_split_none_maxsplit") {
        let m = e["maxsplit"].as_u64().unwrap() as usize;
        assert_eq!(text::split_whitespace(s(&e["in"]), Some(m)), strs(&e["out"]), "{e}");
    }
    for e in sec("str_rsplit_none_maxsplit") {
        let m = e["maxsplit"].as_i64().unwrap();
        let m = (m >= 0).then_some(m as usize);
        assert_eq!(text::rsplit_whitespace(s(&e["in"]), m), strs(&e["out"]), "{e}");
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
        assert_eq!(precommit::zsplit(s(&e["in"])), strs(&e["out"]), "{e}");
    }
    for e in sec("skip_env") {
        let got: Vec<&str> = precommit::get_skips(s(&e["in"])).into_iter().collect();
        assert_eq!(got, strs(&e["out_sorted_set"]), "{e}");
    }
    for e in sec("pre_push_parse") {
        let b = hex(&e["in_hex"]);
        let out = &e["out"];
        match precommit::pre_push_lines(&b) {
            Err(de) => assert_eq!(de.to_string(), s(&out["decode"]["msg"]), "{e}"),
            Ok(it) => {
                let want = out["lines"].as_array().unwrap();
                let mut n = 0;
                for (got, w) in it.zip(want) {
                    n += 1;
                    match got {
                        Ok(l) => assert_eq!(
                            vec![l.local_branch, l.local_sha, l.remote_branch, l.remote_sha],
                            strs(&w["fields"]),
                            "{e}"
                        ),
                        Err(u) => {
                            assert_eq!(u.to_string(), s(&w["msg"]), "{e}");
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
        match (precommit::merge_msg_conflicts(&b), outcome(&e["out"])) {
            (Ok(got), Ok(w)) => assert_eq!(got, strs(w), "{e}"),
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
    for e in sec("rev_line_re") {
        let got = precommit::rev_line_match(s(&e["in"])).map(|g| g.to_vec());
        let want = e["groups"].as_array().map(|_| strs(&e["groups"]));
        assert_eq!(got, want, "{e}");
    }
    for e in sec("rev_line_rewrite") {
        let got = precommit::rev_line_rewrite(s(&e["line"]), s(&e["new_rev"]), e["frozen"].as_str()).unwrap();
        assert_eq!(got, s(&e["out"]), "{e}");
    }
    for e in sec("autoupdate_original_lines") {
        let lines = text::splitlines(s(&e["in"]), true);
        assert_eq!(lines, strs(&e["out"]["lines"]));
        let idxs: Vec<u64> =
            lines.iter().enumerate().filter(|(_, l)| precommit::rev_line_match(l).is_some()).map(|(i, _)| i as u64).collect();
        let want: Vec<u64> = e["out"]["idxs"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect();
        assert_eq!(idxs, want, "{e}");
    }
    for e in sec("migrate_header_split") {
        let (h, r) = precommit::migrate_header_split(s(&e["in"]));
        assert_eq!((h, r), (s(&e["out"]["header"]), s(&e["out"]["rest"])), "{e}");
        assert_eq!(textio::textwrap_indent(r, "    "), s(&e["out"]["indented_rest"]), "{e}");
    }
    for e in sec("textwrap_indent") {
        assert_eq!(textio::textwrap_indent(s(&e["in"]), s(&e["prefix"])), s(&e["out"]), "{e}");
    }
    for e in sec("exe_exists_pure") {
        let fl = if s(&e["flavor"]) == "posix" { precommit::Flavor::Posix } else { precommit::Flavor::Nt };
        assert_eq!(precommit::exe_exists_pure(fl, s(&e["found"]), s(&e["home"])), e["out"].as_bool().unwrap(), "{e}");
    }
}

#[test]
fn rev_line_regex_and_manual_agree_with_python() {
    for name in ["rev_line_re", "random_rev_lines"] {
        for e in sec(name) {
            let want = e["groups"].as_array().map(|_| strs(&e["groups"]));
            let a = precommit::rev_line_match(s(&e["in"])).map(|g| g.to_vec());
            let b = precommit::rev_line_match_manual(s(&e["in"])).map(|g| g.to_vec());
            assert_eq!(a, want, "regex {e}");
            assert_eq!(b, want, "manual {e}");
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
        assert_eq!(textio::windows_text_write(i).as_bytes(), hex(&e["windows_hex"]), "{e}");
        assert_eq!(i.as_bytes(), hex(&e["posix_hex"]), "{e}");
    }
    for e in sec("utf8_decode_strict") {
        let b = hex(&e["in_hex"]);
        match (codec::decode_utf8(&b), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, s(w)),
            (Err(g), Err((t, m))) => {
                assert_eq!(t, "UnicodeDecodeError");
                assert_eq!(g.to_string(), m, "{e}");
            }
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
    for e in sec("utf8_decode_backslashreplace") {
        assert_eq!(codec::decode_utf8_backslashreplace(&hex(&e["in_hex"])), s(&e["out"]), "{e}");
    }
}

#[test]
fn reprs() {
    for e in sec("repr") {
        let t = &e["in"];
        let got = if s(&t["t"]) == "str" {
            match py_str(t) {
                Ok(st) => repr::repr_str(&st),
                Err(bytes) => repr::repr_fsbytes(&bytes),
            }
        } else {
            repr::repr(&untag(t))
        };
        assert_eq!(got, s(&e["out"]), "{e}");
    }
    for e in sec("fstring_eq") {
        assert_eq!(format!("prefix={}", repr::repr_str(&py_str(&e["in"]).unwrap())), s(&e["out"]));
    }
    for e in sec("oserror_str") {
        let a = e["args"].as_array().unwrap();
        if a.len() < 2 {
            continue; // OSError(msg) / OSError() are plain str(args)
        }
        let errno = a[0].as_i64().unwrap() as i32;
        let got = repr::oserror_str(errno, s(&a[1]), a.get(2).and_then(Value::as_str), a.get(4).and_then(Value::as_str));
        assert_eq!(got, s(&e["out"]), "{e}");
    }
    for e in sec("stdlib_calledprocesserror_str") {
        let cmd = &e["cmd"];
        let cmd_str = if s(&cmd["t"]) == "str" { py_str(cmd).unwrap() } else { repr::repr(&untag(cmd)) };
        let rc = e["returncode"].as_i64().unwrap() as i32;
        assert_eq!(repr::stdlib_called_process_error_str(rc, &cmd_str), s(&e["out"]), "{e}");
    }
    for e in sec("keyerror_str") {
        assert_eq!(repr::repr_str(s(&e["in"])), s(&e["out"]));
    }
    for e in sec("argparse_invalid_choice") {
        let choices: Vec<String> = strs(&e["choices"]).iter().map(|c| repr::repr_str(c)).collect();
        let got = format!("invalid choice: {} (choose from {})", repr::repr_str(s(&e["value"])), choices.join(", "));
        assert_eq!(got, s(&e["out"]));
    }
    for e in sec("parse_version") {
        match (int::parse_version(s(&e["in"])), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => {
                let w: Vec<i128> = w.as_array().unwrap().iter().map(|x| i128::from(x.as_i64().unwrap())).collect();
                assert_eq!(g, w, "{e}");
            }
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

#[test]
fn shlex_vectors() {
    for e in sec("shlex_quote") {
        assert_eq!(shlex::quote(s(&e["in"])), s(&e["out"]), "{e}");
    }
    for e in sec("shlex_join") {
        assert_eq!(shlex::join(strs(&e["in"])), s(&e["out"]), "{e}");
    }
    for e in sec("shlex_split") {
        match (shlex::split(s(&e["in"])), outcome(&e["out"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, strs(w), "{e}"),
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

fn pres(r: Result<Vec<u8>, posixpath::PathError>) -> Result<String, String> {
    r.map(|v| String::from_utf8(v).unwrap()).map_err(|e| e.to_string())
}
fn want(v: &Value) -> Result<String, String> {
    outcome(v).map(|x| s(x).to_owned()).map_err(|(_, m)| m)
}

#[test]
fn posixpath_vectors() {
    let b = str::as_bytes;
    let u = |v: &[u8]| String::from_utf8(v.to_vec()).unwrap();
    for e in sec("posixpath_normpath") {
        assert_eq!(u(&posixpath::normpath(b(s(&e["in"])))), s(&e["out"]), "{e}");
    }
    for e in sec("posixpath_abspath") {
        assert_eq!(Ok(u(&posixpath::abspath(b(s(&e["in"])), b(s(&e["cwd"]))))), want(&e["out"]), "{e}");
    }
    for e in sec("posixpath_join") {
        let parts = strs(&e["in"]);
        let rest: Vec<&[u8]> = parts[1..].iter().map(|x| x.as_bytes()).collect();
        assert_eq!(u(&posixpath::join(b(parts[0]), &rest)), s(&e["out"]), "{e}");
    }
    for e in sec("posixpath_relpath") {
        let got = posixpath::relpath(b(s(&e["path"])), e["start"].as_str().map(str::as_bytes), b(s(&e["cwd"])));
        assert_eq!(pres(got), want(&e["out"]), "{e}");
    }
    for e in sec("posixpath_commonpath") {
        let ps: Vec<&[u8]> = strs(&e["in"]).into_iter().map(str::as_bytes).collect();
        assert_eq!(pres(posixpath::commonpath(&ps)), want(&e["out"]), "{e}");
    }
    for e in sec("posixpath_parts") {
        let p = b(s(&e["in"]));
        assert_eq!(u(posixpath::dirname(p)), s(&e["dirname"]), "{e}");
        assert_eq!(u(posixpath::basename(p)), s(&e["basename"]), "{e}");
        let (h, t) = posixpath::split(p);
        assert_eq!(vec![u(h), u(t)], strs(&e["split"]), "{e}");
        let (r, x) = posixpath::splitext(p);
        assert_eq!(vec![u(r), u(x)], strs(&e["splitext"]), "{e}");
        assert_eq!(posixpath::isabs(p), e["isabs"].as_bool().unwrap());
    }
    for e in sec("posixpath_expanduser") {
        let home = e["env"].get("HOME").and_then(Value::as_str).map(str::as_bytes);
        let got = posixpath::expanduser(b(s(&e["in"])), home, || None, |_| None);
        assert_eq!(Ok(u(&got)), want(&e["out"]), "{e}");
    }
    for e in sec("random_posixpath") {
        let (a, bb) = (s(&e["a"]), s(&e["b"]));
        let cwd = b"/home/u/repo/sub";
        assert_eq!(u(&posixpath::normpath(b(a))), s(&e["normpath"]), "{e}");
        assert_eq!(u(&posixpath::join(b(a), &[b(bb)])), s(&e["join"]), "{e}");
        let (h, t) = posixpath::split(b(a));
        assert_eq!(vec![u(h), u(t)], strs(&e["split"]), "{e}");
        let (r, x) = posixpath::splitext(b(a));
        assert_eq!(vec![u(r), u(x)], strs(&e["splitext"]), "{e}");
        assert_eq!(Ok(u(&posixpath::abspath(b(a), cwd))), want(&e["abspath"]), "{e}");
        let start = (!bb.is_empty()).then_some(b(bb));
        assert_eq!(pres(posixpath::relpath(b(a), start, cwd)), want(&e["relpath"]), "{e}");
        assert_eq!(pres(posixpath::commonpath(&[b(a), b(bb)])), want(&e["commonpath"]), "{e}");
    }
}

#[cfg(unix)]
#[test]
fn posixpath_realpath_fixture() {
    use std::os::unix::ffi::OsStrExt;
    let fx = &V["posixpath_realpath_fixture"];
    let tmp = std::env::temp_dir().join(format!("pycompat-realpath-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
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
    for c in fx["cases"].as_array().unwrap() {
        let input = s(&c["in"]).replace("$ROOT", &rs);
        let got = posixpath::realpath(input.as_bytes(), &rootb).unwrap();
        let got = String::from_utf8(got).unwrap().replace(&rs, "$ROOT");
        assert_eq!(got, s(&c["out"]), "{c}");
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn ntpath_vectors() {
    for e in sec("ntpath_normpath") {
        assert_eq!(ntpath::normpath(s(&e["in"])), s(&e["out"]), "{e}");
    }
    for e in sec("ntpath_abspath_fallback") {
        assert_eq!(Ok(ntpath::abspath_fallback(s(&e["in"]), s(&e["cwd"]))), want(&e["out"]), "{e}");
    }
    for e in sec("ntpath_join") {
        let parts = strs(&e["in"]);
        assert_eq!(ntpath::join(parts[0], &parts[1..]), s(&e["out"]), "{e}");
    }
    for e in sec("ntpath_relpath") {
        let got = ntpath::relpath(s(&e["path"]), e["start"].as_str(), s(&e["cwd"])).map_err(|x| x.to_string());
        assert_eq!(got, want(&e["out"]), "{e}");
    }
    for e in sec("ntpath_commonpath") {
        let got = ntpath::commonpath(&strs(&e["in"])).map_err(|x| x.to_string());
        assert_eq!(got, want(&e["out"]), "{e}");
    }
    for e in sec("ntpath_parts") {
        let p = s(&e["in"]);
        assert_eq!(ntpath::dirname(p), s(&e["dirname"]), "{e}");
        assert_eq!(ntpath::basename(p), s(&e["basename"]), "{e}");
        let (h, t) = ntpath::split(p);
        assert_eq!(vec![h.as_str(), t], strs(&e["split"]), "{e}");
        let (r, x) = ntpath::splitext(p);
        assert_eq!(vec![r, x], strs(&e["splitext"]), "{e}");
        let (d, rest) = ntpath::splitdrive(p);
        assert_eq!(vec![d, rest], strs(&e["splitdrive"]), "{e}");
        assert_eq!(ntpath::isabs_312(p), e["isabs"].as_bool().unwrap(), "{e}");
        assert_eq!(ntpath::normcase(p), s(&e["normcase"]), "{e}");
    }
    for e in sec("ntpath_expanduser") {
        let env = e["env"].as_object().unwrap().clone();
        let got = ntpath::expanduser(s(&e["in"]), |k| env.get(k).and_then(Value::as_str).map(str::to_owned));
        assert_eq!(Ok(got), want(&e["out"]), "{e}");
    }
    for e in sec("random_ntpath") {
        let (a, bb) = (s(&e["a"]), s(&e["b"]));
        let cwd = "C:\\Users\\u\\repo\\sub";
        assert_eq!(ntpath::normpath(a), s(&e["normpath"]), "{e}");
        assert_eq!(ntpath::join(a, &[bb]), s(&e["join"]), "{e}");
        let (h, t) = ntpath::split(a);
        assert_eq!(vec![h.as_str(), t], strs(&e["split"]), "{e}");
        let (r, x) = ntpath::splitext(a);
        assert_eq!(vec![r, x], strs(&e["splitext"]), "{e}");
        let (d, rest) = ntpath::splitdrive(a);
        assert_eq!(vec![d, rest], strs(&e["splitdrive"]), "{e}");
        assert_eq!(ntpath::isabs_312(a), e["isabs"].as_bool().unwrap(), "{e}");
        assert_eq!(ntpath::normcase(a), s(&e["normcase"]), "{e}");
        assert_eq!(Ok(ntpath::abspath_fallback(a, cwd)), want(&e["abspath"]), "{e}");
        let start = (!bb.is_empty()).then_some(bb);
        assert_eq!(ntpath::relpath(a, start, cwd).map_err(|x| x.to_string()), want(&e["relpath"]), "{e}");
        assert_eq!(ntpath::commonpath(&[a, bb]).map_err(|x| x.to_string()), want(&e["commonpath"]), "{e}");
    }
}

#[test]
fn adjust_args_and_chdir() {
    for e in sec("adjust_args_and_chdir") {
        let (cwd, top, arg) = (s(&e["cwd"]), s(&e["toplevel"]), s(&e["arg"]));
        let make_abs = s(&e["kind"]) == "files" || e["exists"].as_bool().unwrap();
        let got: Result<String, String> = if s(&e["flavor"]) == "posix" {
            let a = if make_abs { posixpath::abspath(arg.as_bytes(), cwd.as_bytes()) } else { arg.as_bytes().to_vec() };
            pres(posixpath::relpath(&a, None, top.as_bytes()))
        } else {
            let a = if make_abs { ntpath::abspath_fallback(arg, cwd) } else { arg.to_owned() };
            ntpath::relpath(&a, None, top).map_err(|x| x.to_string())
        };
        assert_eq!(got, want(&e["out"]), "{e}");
        if let (Ok(g), Some(cl)) = (&got, e["out"].get("classifier")) {
            assert_eq!(g.replace('\\', "/"), s(cl));
        }
    }
}

#[cfg(unix)]
#[test]
fn find_executable_fixture() {
    use std::os::unix::fs::PermissionsExt;
    let fx = &V["find_executable_posix_fixture"];
    let tmp = std::env::temp_dir().join(format!("pycompat-fe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
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
        let env = c["env"].as_object().unwrap();
        let path = env.get("PATH").and_then(Value::as_str).map(|v| v.replace("$ROOT", &rs));
        let pathext = env.get("PATHEXT").and_then(Value::as_str).map(str::to_owned);
        let got = find_executable(s(&c["exe"]), path.as_deref(), pathext.as_deref(), &root)
            .map(|p| p.replace(&rs, "$ROOT"));
        assert_eq!(got.as_deref(), c["out"].as_str(), "{c}");
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// `parse_shebang.find_executable` (POSIX flavor), relative candidates are
/// checked against `cwd` (snp never chdirs).
#[cfg(unix)]
fn find_executable(exe: &str, path: Option<&str>, pathext: Option<&str>, cwd: &std::path::Path) -> Option<String> {
    let exe = String::from_utf8(posixpath::normpath(exe.as_bytes())).unwrap();
    if exe.contains('/') {
        return Some(exe);
    }
    let mut names: Vec<String> = Vec::new();
    if let Some(exts) = pathext {
        names.extend(exts.split(':').map(|e| format!("{exe}{e}")));
    }
    names.push(exe.clone());
    for dir in path.unwrap_or("").split(':') {
        for name in &names {
            let joined = String::from_utf8(posixpath::join(dir.as_bytes(), &[name.as_bytes()])).unwrap();
            let abs = cwd.join(&joined);
            let is_file = std::fs::metadata(&abs).map(|m| m.is_file()).unwrap_or(false);
            if is_file && rustix::fs::access(&abs, rustix::fs::Access::EXEC_OK).is_ok() {
                return Some(joined);
            }
        }
    }
    None
}

#[test]
fn random_corpora() {
    for e in sec("random_str_ops") {
        let t = s(&e["in"]);
        assert_eq!(text::splitlines(t, false), strs(&e["splitlines"]), "{e}");
        assert_eq!(text::splitlines(t, true), strs(&e["splitlines_keepends"]), "{e}");
        assert_eq!(text::split_whitespace(t, None), strs(&e["split"]), "{e}");
        assert_eq!(text::split_whitespace(t, Some(1)), strs(&e["split_max1"]), "{e}");
        assert_eq!(text::rsplit_whitespace(t, Some(3)), strs(&e["rsplit_max3"]), "{e}");
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
        assert_eq!(textio::textwrap_indent(t, "  "), s(&e["textwrap_indent"]), "{e}");
        let skips: Vec<&str> = precommit::get_skips(t).into_iter().collect();
        assert_eq!(skips, strs(&e["skips"]), "{e}");
    }
    for e in sec("random_bytes_ops") {
        let b = hex(&e["in_hex"]);
        let sl: Vec<Vec<u8>> = text::bytes_splitlines(&b, false).into_iter().map(<[u8]>::to_vec).collect();
        assert_eq!(sl, hexes(&e["splitlines_hex"]), "{e}");
        let sk: Vec<Vec<u8>> = text::bytes_splitlines(&b, true).into_iter().map(<[u8]>::to_vec).collect();
        assert_eq!(sk, hexes(&e["splitlines_keepends_hex"]), "{e}");
        assert_eq!(text::bytes_strip(&b), hex(&e["strip_hex"]), "{e}");
        assert_eq!(text::bytes_rstrip_set(&b, b"\r\n"), hex(&e["rstrip_crlf_hex"]), "{e}");
        match (codec::decode_utf8(&b), outcome(&e["decode"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, s(w)),
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
        assert_eq!(codec::decode_utf8_backslashreplace(&b), s(&e["decode_backslashreplace"]), "{e}");
        match (precommit::merge_msg_conflicts(&b), outcome(&e["merge_conflicts"])) {
            (Ok(g), Ok(w)) => assert_eq!(g, strs(w), "{e}"),
            (Err(g), Err((_, m))) => assert_eq!(g.to_string(), m, "{e}"),
            (g, w) => panic!("{e}: {g:?} vs {w:?}"),
        }
    }
}

#[test]
fn rust_std_pitfalls_are_real() {
    // regex crate `\s` is Unicode White_Space: misses U+001C..U+001F
    let naive = regex::Regex::new(r"^\s$").unwrap();
    assert!(!naive.is_match("\x1f"));
    assert!(precommit::rev_line_match("\x1f rev: v1\n").is_some());
    // str::lines() does not split on \r, \x0b, \x0c, \x1c-\x1e, \x85, U+2028/9
    assert_eq!("a\rb\x0bc".lines().count(), 1);
    assert_eq!(text::splitlines("a\rb\x0bc", false).len(), 3);
    // str::split_whitespace / trim use White_Space (no \x1c-\x1f)
    assert_eq!("a\x1fb".split_whitespace().count(), 1);
    assert_eq!(text::split_whitespace("a\x1fb", None).len(), 2);
    // <[u8]>::trim_ascii omits \x0b
    assert_eq!(b"a\x0b".trim_ascii(), b"a\x0b");
    assert_eq!(text::bytes_strip(b"a\x0b"), b"a");
    // f64 Display is not Python repr
    assert_eq!(format!("{}", 1e16_f64), "10000000000000000");
    assert_eq!(repr::repr_float(1e16), "1e+16");
}
