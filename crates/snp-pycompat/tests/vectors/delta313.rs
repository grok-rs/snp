//! The CPython 3.13.14 delta (`pycompat-delta-cpython313.json`): every entry is classified. Where
//! snp follows 3.13 (UCD 15.1 `isprintable`, `repr`, east-asian width; ADR-020) the 3.13 value is
//! the expectation (asserted by the section tests through `view313`); where snp keeps the 3.12
//! semantics (ntpath `isabs`, the `abspath` fallback, `relpath`, `commonpath`) the 3.13 value is
//! proven to be the 3.13 *composition* of snp's public primitives (`isabs_313`, `splitroot`,
//! `normcase`, `normpath`, `join`). Any other drift fails until it is classified here.

use snp_pycompat::ntpath;

use crate::common::{D, V, s, strs, u8s, want};
use crate::support::json::Value;

/// CPython 3.13 `ntpath._abspath_fallback`: the 3.13 `isabs` decides whether `cwd` is joined.
fn abspath_313(p: &[u8], cwd: &[u8]) -> Vec<u8> {
    if ntpath::isabs_313(p) {
        ntpath::normpath(p)
    } else {
        ntpath::normpath(&ntpath::join(cwd, &[p]))
    }
}

/// CPython 3.13 `ntpath.relpath` over the fallback `abspath`: no `normpath` before `abspath`, no
/// filtering of the split components, and `sep.join(rel_list)` instead of `join(*rel_list)`.
fn relpath_313(p: &[u8], start: Option<&[u8]>, cwd: &[u8]) -> Result<String, String> {
    if p.is_empty() {
        return Err("no path specified".to_owned());
    }
    let start_abs = abspath_313(start.unwrap_or(b"."), cwd);
    let path_abs = abspath_313(p, cwd);
    let (sd, _, sr) = ntpath::splitroot(&start_abs);
    let (pd, _, pr) = ntpath::splitroot(&path_abs);
    if ntpath::normcase(sd) != ntpath::normcase(pd) {
        return Err(format!(
            "path is on mount {}, start on mount {}",
            snp_pycompat::repr::repr_str(&u8s(pd)),
            snp_pycompat::repr::repr_str(&u8s(sd))
        ));
    }
    let parts = |r: &[u8]| -> Vec<Vec<u8>> {
        if r.is_empty() {
            Vec::new()
        } else {
            r.split(|&b| b == b'\\').map(<[u8]>::to_vec).collect()
        }
    };
    let (sl, pl) = (parts(sr), parts(pr));
    let i = sl
        .iter()
        .zip(&pl)
        .take_while(|(a, b)| ntpath::normcase(a) == ntpath::normcase(b))
        .count();
    let mut rel: Vec<Vec<u8>> = vec![b"..".to_vec(); sl.len() - i];
    rel.extend(pl[i..].iter().cloned());
    if rel.is_empty() {
        return Ok(".".to_owned());
    }
    Ok(u8s(&rel.join(&b'\\')))
}

/// CPython 3.13 `ntpath.commonpath`: drives are checked before roots, and the texts changed.
fn commonpath_313(ps: &[&[u8]]) -> Result<String, String> {
    if ps.is_empty() {
        return Err("commonpath() arg is an empty iterable".to_owned());
    }
    let lowered: Vec<Vec<u8>> = ps.iter().map(|p| ntpath::normcase(p)).collect();
    let splits: Vec<(&[u8], &[u8], &[u8])> = lowered.iter().map(|p| ntpath::splitroot(p)).collect();
    if splits.iter().any(|x| x.0 != splits[0].0) {
        return Err("Paths don't have the same drive".to_owned());
    }
    let first: Vec<u8> = ps[0]
        .iter()
        .map(|&b| if b == b'/' { b'\\' } else { b })
        .collect();
    let (drive, root, path) = ntpath::splitroot(&first);
    if splits.iter().any(|x| x.1 != splits[0].1) {
        return Err(if drive.is_empty() {
            "Can't mix rooted and not-rooted paths".to_owned()
        } else {
            "Can't mix absolute and relative paths".to_owned()
        });
    }
    let keep = |c: &&[u8]| !c.is_empty() && *c != b".";
    let common: Vec<&[u8]> = path.split(|&b| b == b'\\').filter(keep).collect();
    let sp: Vec<Vec<&[u8]>> = splits
        .iter()
        .map(|x| x.2.split(|&b| b == b'\\').filter(keep).collect())
        .collect();
    let (s1, s2) = (sp.iter().min().unwrap(), sp.iter().max().unwrap());
    let n = s1.iter().zip(s2).take_while(|(a, b)| a == b).count();
    let mut out = drive.to_vec();
    out.extend_from_slice(root);
    out.extend_from_slice(&common[..n].join(&b'\\'));
    Ok(u8s(&out))
}

/// The fields of a 3.12 entry that the 3.13 entry changes.
fn changed_fields<'a>(base: &'a Value, new: &'a Value) -> Vec<&'a str> {
    match (base.as_object(), new.as_object()) {
        (Some(b), Some(n)) => {
            assert_eq!(
                b.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                n.iter().map(|(k, _)| k).collect::<Vec<_>>()
            );
            n.iter()
                .filter(|(k, v)| base.get(k) != Some(v))
                .map(|(k, _)| k.as_str())
                .collect()
        }
        _ => vec!["*"],
    }
}

fn check_ntpath_field(section: &str, entry: &Value, field: &str) {
    let nt_cwd = b"C:\\Users\\u\\repo\\sub";
    match (section, field) {
        ("ntpath_parts" | "random_ntpath" | "ntpath_more", "isabs") => {
            let p = s(entry.get("in").or_else(|| entry.get("a")).unwrap());
            assert_eq!(
                ntpath::isabs_313(p.as_bytes()),
                entry["isabs"].as_bool().unwrap(),
                "{entry}"
            );
            let w: Vec<u16> = p.encode_utf16().collect();
            assert_eq!(
                ntpath::isabs_313(&w),
                entry["isabs"].as_bool().unwrap(),
                "{entry}"
            );
        }
        ("ntpath_abspath_fallback", "out") => {
            let got = u8s(&abspath_313(
                s(&entry["in"]).as_bytes(),
                s(&entry["cwd"]).as_bytes(),
            ));
            assert_eq!(Ok(got), want(&entry["out"]), "{entry}");
        }
        ("random_ntpath", "abspath") => {
            let got = u8s(&abspath_313(s(&entry["a"]).as_bytes(), nt_cwd));
            assert_eq!(Ok(got), want(&entry["abspath"]), "{entry}");
        }
        ("random_ntpath", "relpath") => {
            let (a, b) = (s(&entry["a"]), s(&entry["b"]));
            let start = (!b.is_empty()).then_some(b.as_bytes());
            assert_eq!(
                relpath_313(a.as_bytes(), start, nt_cwd),
                want(&entry["relpath"]),
                "{entry}"
            );
        }
        ("ntpath_pairs", "relpath") => {
            let got = relpath_313(
                s(&entry["a"]).as_bytes(),
                Some(s(&entry["b"]).as_bytes()),
                s(&entry["cwd"]).as_bytes(),
            );
            assert_eq!(got, want(&entry["relpath"]), "{entry}");
        }
        ("random_ntpath" | "ntpath_pairs", "commonpath") => {
            let (a, b) = (s(&entry["a"]), s(&entry["b"]));
            assert_eq!(
                commonpath_313(&[a.as_bytes(), b.as_bytes()]),
                want(&entry["commonpath"]),
                "{entry}"
            );
        }
        ("ntpath_commonpath", "out") => {
            let ps: Vec<&[u8]> = strs(&entry["in"]).into_iter().map(str::as_bytes).collect();
            assert_eq!(commonpath_313(&ps), want(&entry["out"]), "{entry}");
        }
        _ => panic!("unclassified CPython 3.13 drift: {section}.{field} in {entry}"),
    }
}

/// Every entry of the delta is either a UCD 15.1 section snp follows, or a 3.12-pinned ntpath
/// function whose 3.13 value the public primitives reproduce.
#[test]
fn every_313_drift_is_classified() {
    assert_eq!(s(&D["meta"]["python"]), "3.13.14");
    assert_eq!(s(&D["meta"]["unicode"]), "15.1.0");
    assert_eq!(s(&D["meta"]["base_python"]), s(&V["meta"]["python"]));
    assert_eq!(s(&V["meta"]["python"]), "3.12.3");
    let delta = D["delta"].as_object().unwrap();
    assert_ne!(delta.len(), 0);
    let mut followed = 0_usize;
    let mut composed = 0_usize;
    for (section, entries) in delta {
        match section.as_str() {
            // snp follows UCD 15.1 here (ADR-020): the section tests use the 3.13 view
            "sets" => {
                let keys: Vec<&str> = entries
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect();
                assert_eq!(keys, ["str_isprintable_false"], "only isprintable moves");
                followed += 1;
            }
            "east_asian_width_wf" | "repr_codepoints" | "len_cjk" => followed += 1,
            _ => {
                let base = V[section.as_str()].as_array().unwrap();
                for (k, new) in entries.as_object().unwrap() {
                    let i: usize = k.parse().unwrap();
                    for field in changed_fields(&base[i], new) {
                        check_ntpath_field(section, new, field);
                        composed += 1;
                    }
                }
            }
        }
    }
    assert!(followed >= 2 && composed > 0, "{followed} {composed}");
}
