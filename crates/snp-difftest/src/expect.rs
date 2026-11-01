//! The `expected:` checks of divergence files: how each side must behave for a registered divergence to count as
//! reproduced (ADR-071; WBS "Divergence files").
//!
//! A check is `key: value`. Stream keys apply to `stdout`, `stderr`, `config_file` (`.pre-commit-config.yaml` after
//! the step) and `git_config` (`.git/config`): `<stream>` (exact bytes), `<stream>_contains`, `<stream>_not_contains`,
//! `<stream>_starts_with`, `<stream>_ends_with`, `<stream>_matches` (Python `re.search`, see [`crate::minire`]). A
//! trailing `_<n>` repeats a key (`stdout_contains_2`). Other keys: `exit`, `exit_not`, `timed_out`,
//! `git_config_unchanged`, `tree: {path: bytes}`, `tree_has: [path]`, `tree_missing: [path]`, `absent: [absolute
//! path]`. A value may be `{compat: …, snp: …}` to differ per brand. `${…}` substitutions of the materialiser apply to
//! every text value except `_matches` patterns; in snp-brand runs, plain text values of snp's side are passed through
//! the brand map (they are written in compat texts). An unknown key is a failure, so a typo cannot pass silently.
//!
//! Owner: WP-35.

use crate::exec::Captured;
use crate::materialise::World;
use crate::minire::Regex;
use crate::normalise::BrandMap;
use crate::scenario::Brand;
use crate::snapshot::{Entry, State};
use crate::text::{contains_bytes, show};
use crate::yamlv::Yv;

/// What one check sees.
#[derive(Debug)]
pub(crate) struct CheckCx<'a> {
    /// The step's streams and exit.
    pub(crate) cap: &'a Captured,
    /// The state after the step.
    pub(crate) state: &'a State,
    /// `.git/config` before the first step.
    pub(crate) initial_git_config: Option<&'a [u8]>,
    /// Substitutions.
    pub(crate) world: &'a World,
    /// The run's brand.
    pub(crate) brand: Brand,
    /// The brand map to apply to plain text values (snp's side of snp-brand runs only).
    pub(crate) map: Option<&'a BrandMap>,
}

/// Evaluates `checks`; returns one message per failed check (empty = all hold).
pub(crate) fn evaluate(checks: &[(String, Yv)], cx: &CheckCx<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for (key, value) in checks {
        if let Err(e) = check(key, value, cx) {
            out.push(format!("{key}: {e}"));
        }
    }
    out
}

/// A `{compat: …, snp: …}` value resolved for `brand`; other values unchanged.
fn by_brand(v: &Yv, brand: Brand) -> &Yv {
    if let Some(m) = v.as_map()
        && !m.is_empty()
        && m.iter().all(|(k, _)| k == "compat" || k == "snp")
    {
        return v.get(brand.name()).unwrap_or(&Yv::Null);
    }
    v
}

/// The bytes of a text value after substitution (and the brand map when asked).
fn text_value(v: &Yv, cx: &CheckCx<'_>, mapped: bool) -> Result<Vec<u8>, String> {
    let raw = v.to_bytes().ok_or_else(|| format!("expected a string, got a {}", v.kind()))?;
    let b = cx.world.subst_bytes(&raw)?;
    Ok(match (mapped, cx.map) {
        (true, Some(m)) => m.apply(&b),
        _ => b,
    })
}

fn int_value(v: &Yv) -> Result<i64, String> {
    v.as_i64().ok_or_else(|| format!("expected an integer, got {v:?}"))
}

fn path_list(v: &Yv, cx: &CheckCx<'_>) -> Result<Vec<String>, String> {
    let items: Vec<&Yv> = match v {
        Yv::Seq(items) => items.iter().collect(),
        other => vec![other],
    };
    items
        .into_iter()
        .map(|i| i.to_text().ok_or_else(|| String::from("expected a list of paths")).and_then(|t| cx.world.subst(&t)))
        .collect()
}

/// Strips a trailing `_<digits>` repetition suffix.
fn base_key(key: &str) -> &str {
    match key.rsplit_once('_') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => key,
    }
}

fn stream<'a>(name: &str, cx: &'a CheckCx<'_>) -> Option<Option<&'a [u8]>> {
    match name {
        "stdout" => Some(Some(&cx.cap.stdout)),
        "stderr" => Some(Some(&cx.cap.stderr)),
        "config_file" => Some(cx.state.config_file.as_deref()),
        "git_config" => Some(cx.state.git_config.as_deref()),
        _ => None,
    }
}

fn shown(b: &[u8]) -> String {
    let s = show(b);
    if s.chars().count() > 2000 {
        let head: String = s.chars().take(2000).collect();
        format!("{head:?}…")
    } else {
        format!("{s:?}")
    }
}

fn check(key: &str, value: &Yv, cx: &CheckCx<'_>) -> Result<(), String> {
    let v = by_brand(value, cx.brand);
    let explicit_brand = !std::ptr::eq(v, value);
    let mapped = !explicit_brand;
    let key = base_key(key);
    match key {
        "exit" => {
            let want = int_value(v)?;
            let got = cx.cap.exit.map(i64::from);
            if got == Some(want) { Ok(()) } else { Err(format!("want {want}, got {got:?}")) }
        }
        "exit_not" => {
            let not = int_value(v)?;
            let got = cx.cap.exit.map(i64::from);
            if got == Some(not) { Err(format!("got {not}")) } else { Ok(()) }
        }
        "timed_out" => {
            let want = v.as_bool().ok_or("expected a boolean")?;
            if cx.cap.timed_out == want { Ok(()) } else { Err(format!("want {want}, got {}", cx.cap.timed_out)) }
        }
        "git_config_unchanged" => {
            let want = v.as_bool().ok_or("expected a boolean")?;
            let same = cx.state.git_config.as_deref() == cx.initial_git_config;
            if same == want { Ok(()) } else { Err(format!("unchanged = {same}")) }
        }
        "tree" => {
            let m = v.as_map().ok_or("expected a mapping path → contents")?;
            for (p, c) in m {
                let want = text_value(c, cx, false)?;
                match cx.state.tree.get(p.as_bytes()) {
                    Some(Entry::File { bytes, .. }) if *bytes == want => {}
                    Some(Entry::File { bytes, .. }) => {
                        return Err(format!("{p}: want {}, got {}", shown(&want), shown(bytes)));
                    }
                    Some(other) => return Err(format!("{p}: not a regular file ({other:?})")),
                    None => return Err(format!("{p}: missing")),
                }
            }
            Ok(())
        }
        "tree_has" | "tree_missing" => {
            let want_present = key == "tree_has";
            for p in path_list(v, cx)? {
                let present = cx.state.tree.contains_key(p.as_bytes());
                if present != want_present {
                    return Err(format!("{p}: {}", if present { "present" } else { "missing" }));
                }
            }
            Ok(())
        }
        "absent" => {
            for p in path_list(v, cx)? {
                if std::fs::symlink_metadata(&p).is_ok() {
                    return Err(format!("{p} exists"));
                }
            }
            Ok(())
        }
        _ => stream_check(key, v, cx, mapped),
    }
}

fn stream_check(key: &str, v: &Yv, cx: &CheckCx<'_>, mapped: bool) -> Result<(), String> {
    const OPS: [&str; 5] = ["_not_contains", "_contains", "_starts_with", "_ends_with", "_matches"];
    let (name, op) = OPS
        .iter()
        .find_map(|op| key.strip_suffix(op).map(|n| (n, *op)))
        .unwrap_or((key, ""));
    let Some(got) = stream(name, cx) else { return Err(String::from("unknown check")) };
    if op == "_matches" {
        let pat = v.to_text().ok_or("expected a pattern")?;
        let re = Regex::new(&pat).map_err(|e| format!("bad pattern {pat:?}: {e}"))?;
        let got = got.ok_or("absent")?;
        let text = show(got);
        return if re.search(&text)? { Ok(()) } else { Err(format!("{pat:?} does not match {}", shown(got))) };
    }
    let want = text_value(v, cx, mapped)?;
    let Some(got) = got else {
        return if op == "_not_contains" { Ok(()) } else { Err(String::from("absent")) };
    };
    let ok = match op {
        "" => got == want.as_slice(),
        "_contains" => contains_bytes(got, &want),
        "_not_contains" => !contains_bytes(got, &want),
        "_starts_with" => got.starts_with(&want),
        _ => got.ends_with(&want),
    };
    if ok { Ok(()) } else { Err(format!("want {}{}, got {}", op.trim_start_matches('_'), shown(&want), shown(got))) }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use snp_core::env::Env;

    use super::*;
    use crate::materialise::Layout;
    use crate::scenario::Work;

    fn world() -> World {
        World {
            layout: Layout::new(Path::new("/r"), &Work::default()),
            env: Env::from_pairs::<&str, &str>([]),
            git: PathBuf::from("git"),
            sh: None,
            tick: 0,
            vars: vec![(String::from("work"), String::from("/r/work"))],
        }
    }

    fn run(yaml: &str, cap: &Captured, state: &State, brand: Brand, map: Option<&BrandMap>) -> Vec<String> {
        let w = world();
        let doc = Yv::parse(yaml).unwrap();
        let checks = doc.as_map().unwrap().to_vec();
        let cx = CheckCx { cap, state, initial_git_config: Some(b"[core]\n"), world: &w, brand, map };
        evaluate(&checks, &cx)
    }

    #[test]
    fn stream_and_exit_checks() {
        let cap = Captured {
            stdout: b"Check the log at /r/work/store/pre-commit.log\n".to_vec(),
            stderr: b"saved in /p/x; apply /p/x\n".to_vec(),
            exit: Some(3),
            timed_out: false,
        };
        let mut state = State { config_file: Some(b"repos: []\n".to_vec()), ..State::default() };
        state.git_config = Some(b"[core]\n".to_vec());
        state.tree.insert(b"foo".to_vec(), Entry::File { bytes: b"1\n".to_vec(), exec: false });
        let ok = "exit: 3\nexit_not: 1\ntimed_out: false\nstdout_ends_with: \"${work}/store/pre-commit.log\\n\"\n\
                  stdout_contains: Check\nstdout_contains_2: log\nstdout_not_contains: nope\n\
                  stderr_matches: '^saved in (\\S+); apply \\1\\n$'\nconfig_file: \"repos: []\\n\"\n\
                  git_config_unchanged: true\ntree: {foo: \"1\\n\"}\ntree_has: [foo]\ntree_missing: [bar]\n\
                  absent: [\"${work}/definitely/not/here\"]\n";
        assert_eq!(run(ok, &cap, &state, Brand::Compat, None), Vec::<String>::new());
        let bad = "exit: 1\nstdout_starts_with: nope\nbogus_key: 1\ntree: {foo: x}\nconfig_file_contains: zzz\n";
        assert_eq!(run(bad, &cap, &state, Brand::Compat, None).len(), 5);
    }

    #[test]
    fn brand_values_and_map() {
        let cap = Captured { stdout: b"Check the log at /s/snp.log\n".to_vec(), ..Captured::default() };
        let state = State::default();
        let per_brand = "stdout_ends_with: {compat: \"/pre-commit.log\\n\", snp: \"/snp.log\\n\"}\n";
        assert!(run(per_brand, &cap, &state, Brand::Snp, None).is_empty());
        assert_eq!(run(per_brand, &cap, &state, Brand::Compat, None).len(), 1);
        let map = BrandMap::workspace().unwrap();
        let plain = "stdout_ends_with: \"/pre-commit.log\\n\"\n";
        assert!(run(plain, &cap, &state, Brand::Snp, Some(&map)).is_empty());
        assert_eq!(run(plain, &cap, &state, Brand::Snp, None).len(), 1);
    }
}
