//! Loading the committed vector files and decoding their conventions (15 §A1): bytes as lowercase
//! hex in `*_hex` fields, strings with lone surrogates as code-point lists (`v_cps`), outcomes as
//! `{"ok": x}` / `{"error": T, "msg": m}`, tagged `repr` values, and the 3.13 delta overlay.

use std::path::PathBuf;
use std::sync::LazyLock;

use snp_pycompat::{precommit, repr, text};

use crate::support::json::Value;

fn load(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(name);
    Value::parse(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

/// `pycompat-vectors.cpython312.json`.
pub(crate) static V: LazyLock<Value> = LazyLock::new(|| load("pycompat-vectors.cpython312.json"));

/// `pycompat-delta-cpython313.json`.
pub(crate) static D: LazyLock<Value> = LazyLock::new(|| load("pycompat-delta-cpython313.json"));

/// A section of the 3.12 file.
pub(crate) fn sec(name: &str) -> &'static Vec<Value> {
    V[name]
        .as_array()
        .unwrap_or_else(|| panic!("missing section {name}"))
}

/// The CPython 3.13 view of a section: the 3.12 section with the delta applied (`"*"` replaces the
/// section; otherwise list indices or object keys are replaced).
pub(crate) fn view313(name: &str) -> Value {
    let base = V[name].clone();
    let Some(delta) = D["delta"].get(name) else {
        return base;
    };
    if let Some(all) = delta.get("*") {
        return all.clone();
    }
    match base {
        Value::Array(mut items) => {
            for (k, v) in delta.as_object().unwrap() {
                let i: usize = k.parse().unwrap();
                items[i] = v.clone();
            }
            Value::Array(items)
        }
        Value::Object(mut kv) => {
            for (k, v) in delta.as_object().unwrap() {
                let slot = kv.iter_mut().find(|(n, _)| n == k).unwrap();
                slot.1 = v.clone();
            }
            Value::Object(kv)
        }
        other => panic!("delta for scalar section {name}: {other}"),
    }
}

pub(crate) fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

pub(crate) fn strs(v: &Value) -> Vec<&str> {
    v.as_array().unwrap().iter().map(s).collect()
}

pub(crate) fn hex(v: &Value) -> Vec<u8> {
    let h = s(v);
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2).unwrap(), 16).unwrap())
        .collect()
}

pub(crate) fn hexes(v: &Value) -> Vec<Vec<u8>> {
    v.as_array().unwrap().iter().map(hex).collect()
}

pub(crate) fn u32_of(v: &Value) -> u32 {
    u32::try_from(v.as_u64().unwrap()).unwrap()
}

pub(crate) fn usize_of(v: &Value) -> usize {
    usize::try_from(v.as_u64().unwrap()).unwrap()
}

pub(crate) fn ranges(v: &Value) -> Vec<(u32, u32)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| (u32_of(&r[0]), u32_of(&r[1])))
        .collect()
}

pub(crate) fn in_ranges(cp: u32, rs: &[(u32, u32)]) -> bool {
    rs.iter().any(|&(a, b)| a <= cp && cp <= b)
}

/// Python `{'ok': x}` / `{'error': T, 'msg': m}` → `Result<&Value, (T, m)>`.
pub(crate) fn outcome(v: &Value) -> Result<&Value, (String, String)> {
    v.get("ok")
        .ok_or_else(|| (s(&v["error"]).to_owned(), s(&v["msg"]).to_owned()))
}

/// An outcome whose `ok` value is a string, with the error reduced to its message.
pub(crate) fn want(v: &Value) -> Result<String, String> {
    outcome(v).map(|x| s(x).to_owned()).map_err(|(_, m)| m)
}

/// A vector string: valid text, or the `surrogateescape` bytes of a string with lone surrogates.
#[derive(Debug)]
pub(crate) enum PyStr {
    Text(String),
    Bytes(Vec<u8>),
}

impl PyStr {
    pub(crate) fn text(self) -> String {
        match self {
            Self::Text(t) => t,
            Self::Bytes(b) => panic!("surrogates in {b:?}"),
        }
    }
}

/// A tagged `{"t": "str", "v" | "v_cps": …}` string.
pub(crate) fn py_str(t: &Value) -> PyStr {
    if let Some(v) = t.get("v") {
        return PyStr::Text(s(v).to_owned());
    }
    let mut b = Vec::new();
    for cp in t["v_cps"].as_array().unwrap().iter().map(u32_of) {
        if (0xdc80..=0xdcff).contains(&cp) {
            b.push(u8::try_from(cp - 0xdc00).unwrap());
        } else {
            let mut buf = [0_u8; 4];
            b.extend_from_slice(char::from_u32(cp).unwrap().encode_utf8(&mut buf).as_bytes());
        }
    }
    PyStr::Bytes(b)
}

/// A tagged value as a [`repr::PyValue`].
pub(crate) fn untag(t: &Value) -> repr::PyValue {
    use repr::{PyInt, PyValue as P};
    match s(&t["t"]) {
        "none" => P::None,
        "bool" => P::Bool(t["v"].as_bool().unwrap()),
        "int" => P::Int(
            t["v"]
                .as_i128()
                .map_or_else(|| PyInt::Decimal(t["v"].to_string().into()), PyInt::Small),
        ),
        "float" => P::Float(s(&t["v"]).parse().unwrap()),
        "str" => P::Str(py_str(t).text()),
        "list" => P::List(t["v"].as_array().unwrap().iter().map(untag).collect()),
        "tuple" => P::Tuple(t["v"].as_array().unwrap().iter().map(untag).collect()),
        "dict" => P::Dict(
            t["v"]
                .as_array()
                .unwrap()
                .iter()
                .map(|kv| (untag(&kv[0]), untag(&kv[1])))
                .collect(),
        ),
        other => panic!("tag {other}"),
    }
}

/// `repr()` of a tagged value (strings with surrogates through the `fsdecode` view).
pub(crate) fn repr_of(t: &Value) -> String {
    if s(&t["t"]) == "str" {
        match py_str(t) {
            PyStr::Text(st) => repr::repr_str(&st),
            PyStr::Bytes(bytes) => repr::repr_fsbytes(&bytes),
        }
    } else {
        repr::repr(&untag(t))
    }
}

pub(crate) fn lines(t: &str, keepends: bool) -> Vec<&str> {
    if keepends {
        text::splitlines_keepends(t).collect()
    } else {
        text::splitlines(t).collect()
    }
}

pub(crate) fn blines(b: &[u8], keepends: bool) -> Vec<Vec<u8>> {
    let it: Vec<&[u8]> = if keepends {
        text::bytes_splitlines_keepends(b).collect()
    } else {
        text::bytes_splitlines(b).collect()
    };
    it.into_iter().map(<[u8]>::to_vec).collect()
}

pub(crate) fn u8s(v: &[u8]) -> String {
    String::from_utf8(v.to_vec()).unwrap()
}

pub(crate) fn pres(r: Result<Vec<u8>, snp_pycompat::PathError>) -> Result<String, String> {
    r.map(|v| u8s(&v)).map_err(|e| e.to_string())
}

/// `sorted(_get_skips(SKIP))`.
pub(crate) fn skips(t: &str) -> Vec<String> {
    let mut v: Vec<String> = precommit::get_skips(Some(std::ffi::OsStr::new(t)))
        .into_iter()
        .map(|o| o.into_string().unwrap())
        .collect();
    v.sort();
    v
}

/// pre-commit's `_parse_merge_msg_for_conflicts`: snp's byte parse, then the strict per-line decode
/// pre-commit does (snp never decodes, D-CRASH-12); the first undecodable line is pre-commit's
/// exception.
pub(crate) fn merge_conflicts(b: &[u8]) -> Result<Vec<String>, String> {
    precommit::merge_msg_conflicts(b)
        .into_iter()
        .map(|l| {
            snp_pycompat::codec::decode_utf8(l)
                .map(str::to_owned)
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// `f64` from the 16 hex digits of its IEEE-754 bit pattern (`struct.pack('>d', x).hex()`).
pub(crate) fn f64_of_bits(h: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(h, 16).unwrap())
}

/// The POSIX `posixpath` view used by several sections.
pub(crate) fn by(x: &str) -> &[u8] {
    x.as_bytes()
}
