//! `ntpath` (CPython 3.12) — Windows path semantics, testable on any host.
//!
//! Prototype on `&str`; production code should run the same algorithms on
//! `OsStr::encode_wide()` units (lone surrogates pass through untouched).
//! `abspath_fallback`/`relpath` use the *fallback* used by CPython when the
//! `nt` module is absent; on real Windows CPython calls
//! `GetFullPathNameW(normpath(p))` — use `std::path::absolute(normpath(p))`.
use crate::posixpath::PathError;

const SEP: char = '\\';

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// `ntpath.splitroot(p)` (3.12): `(drive, root, tail)`.
#[must_use]
pub fn splitroot(p: &str) -> (&str, &str, &str) {
    let normp: String = p.replace('/', "\\");
    let nb = normp.as_bytes();
    if nb.first() == Some(&b'\\') {
        if nb.get(1) == Some(&b'\\') {
            // UNC (\\server\share, \\?\UNC\server\share) or device (\\.\dev, \\?\C:)
            let start = if normp.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("\\\\?\\UNC\\")) { 8 } else { 2 };
            let Some(index) = normp.get(start..).and_then(|s| s.find('\\')).map(|i| i + start) else {
                return (p, "", "");
            };
            let Some(index2) = normp.get(index + 1..).and_then(|s| s.find('\\')).map(|i| i + index + 1) else {
                return (p, "", "");
            };
            return (
                p.get(..index2).unwrap_or_default(),
                p.get(index2..=index2).unwrap_or_default(),
                p.get(index2 + 1..).unwrap_or_default(),
            );
        }
        return ("", p.get(..1).unwrap_or_default(), p.get(1..).unwrap_or_default());
    }
    // `normp[1:2] == ':'` — index 1 is a code point index in Python
    let mut ci = p.char_indices();
    if let (Some(_), Some((i1, ':'))) = (ci.next(), ci.next()) {
        let d_end = i1 + 1;
        let drive = p.get(..d_end).unwrap_or_default();
        let after = p.get(d_end..).unwrap_or_default();
        if after.starts_with(is_sep) {
            return (drive, after.get(..1).unwrap_or_default(), after.get(1..).unwrap_or_default());
        }
        return (drive, "", after);
    }
    ("", "", p)
}

/// `ntpath.splitdrive(p)`.
#[must_use]
pub fn splitdrive(p: &str) -> (&str, &str) {
    let (d, _, _) = splitroot(p);
    p.split_at(d.len())
}

/// `ntpath.isabs(p)` with CPython 3.12 semantics (`/x` and `\x` are absolute;
/// 3.13+ requires a drive or UNC).
#[must_use]
pub fn isabs_312(p: &str) -> bool {
    let s: String = p.chars().take(3).map(|c| if c == '/' { '\\' } else { c }).collect();
    s.starts_with('\\') || s.chars().nth(1) == Some(':') && s.chars().nth(2) == Some('\\')
}

/// `ntpath.isabs(p)` with CPython 3.13+ semantics.
#[must_use]
pub fn isabs_313(p: &str) -> bool {
    let s: String = p.chars().take(3).map(|c| if c == '/' { '\\' } else { c }).collect();
    s.starts_with("\\\\") || s.chars().nth(1) == Some(':') && s.chars().nth(2) == Some('\\')
}

/// `ntpath.normpath(p)` (3.12 C implementation semantics).
#[must_use]
pub fn normpath(p: &str) -> String {
    let path = p.replace('/', "\\");
    let (drive, root, rest) = splitroot(&path);
    let mut comps: Vec<&str> = Vec::new();
    for c in rest.split(SEP) {
        if c.is_empty() || c == "." {
            continue;
        }
        if c == ".." {
            match comps.last() {
                Some(&last) if last != ".." => {
                    comps.pop();
                }
                None if !root.is_empty() => {}
                _ => comps.push(c),
            }
        } else {
            comps.push(c);
        }
    }
    let mut out = String::with_capacity(p.len());
    out.push_str(drive);
    out.push_str(root);
    if drive.is_empty() && root.is_empty() && comps.is_empty() {
        out.push('.');
    }
    out.push_str(&comps.join("\\"));
    out
}

/// `ntpath.join(path, *paths)` (3.12).
#[must_use]
pub fn join(path: &str, paths: &[&str]) -> String {
    let (d, r, t) = splitroot(path);
    let (mut rd, mut rr, mut rp) = (d.to_owned(), r.to_owned(), t.to_owned());
    for p in paths {
        let (pd, pr, pp) = splitroot(p);
        if !pr.is_empty() {
            if !pd.is_empty() || rd.is_empty() {
                rd = pd.to_owned();
            }
            rr = pr.to_owned();
            rp = pp.to_owned();
            continue;
        } else if !pd.is_empty() && pd != rd {
            if pd.to_lowercase() != rd.to_lowercase() {
                rd = pd.to_owned();
                rr = pr.to_owned();
                rp = pp.to_owned();
                continue;
            }
            rd = pd.to_owned();
        }
        if rp.chars().last().is_some_and(|c| !is_sep(c)) {
            rp.push(SEP);
        }
        rp.push_str(pp);
    }
    if !rp.is_empty() && rr.is_empty() && rd.chars().last().is_some_and(|c| !matches!(c, ':' | '\\' | '/')) {
        return format!("{rd}\\{rp}");
    }
    format!("{rd}{rr}{rp}")
}

/// `ntpath.split(p)`.
#[must_use]
pub fn split(p: &str) -> (String, &str) {
    let (d, r, rest) = splitroot(p);
    let i = rest.rfind(is_sep).map_or(0, |i| i + 1);
    let (head, tail) = rest.split_at(i);
    (format!("{d}{r}{}", head.trim_end_matches(is_sep)), tail)
}

/// `ntpath.dirname(p)`.
#[must_use]
pub fn dirname(p: &str) -> String {
    split(p).0
}

/// `ntpath.basename(p)`.
#[must_use]
pub fn basename(p: &str) -> &str {
    split(p).1
}

/// `ntpath.splitext(p)`.
#[must_use]
pub fn splitext(p: &str) -> (&str, &str) {
    let (a, _) = crate::posixpath::splitext_generic(p.as_bytes(), b"\\/");
    // the split point is an ASCII '.' or the end, hence a char boundary
    p.split_at_checked(a.len()).unwrap_or((p, ""))
}

/// `ntpath.normcase(p)` — fallback form `replace('/', '\\').lower()`. On
/// Windows CPython uses `LCMapStringEx(LOCALE_NAME_INVARIANT, LCMAP_LOWERCASE)`.
#[must_use]
pub fn normcase(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// `ntpath.abspath(p)` as CPython computes it when `nt` is unavailable
/// (`_abspath_fallback`), with CPython 3.12 `isabs`.
#[must_use]
pub fn abspath_fallback(p: &str, cwd: &str) -> String {
    if isabs_312(p) { normpath(p) } else { normpath(&join(cwd, &[p])) }
}

/// `ntpath.relpath(path, start)` using `abspath_fallback`.
pub fn relpath(path: &str, start: Option<&str>, cwd: &str) -> Result<String, PathError> {
    if path.is_empty() {
        return Err(PathError::NoPathSpecified);
    }
    let start_abs = abspath_fallback(&normpath(start.unwrap_or(".")), cwd);
    let path_abs = abspath_fallback(&normpath(path), cwd);
    let (sd, _, sr) = splitroot(&start_abs);
    let (pd, _, pr) = splitroot(&path_abs);
    if normcase(sd) != normcase(pd) {
        return Err(PathError::DifferentMounts { path_drive: pd.to_owned(), start_drive: sd.to_owned() });
    }
    let sl: Vec<&str> = sr.split(SEP).filter(|c| !c.is_empty()).collect();
    let pl: Vec<&str> = pr.split(SEP).filter(|c| !c.is_empty()).collect();
    let i = sl.iter().zip(&pl).take_while(|(a, b)| normcase(a) == normcase(b)).count();
    let mut rel: Vec<&str> = vec![".."; sl.len() - i];
    rel.extend(pl.get(i..).unwrap_or_default());
    match rel.split_first() {
        None => Ok(".".to_owned()),
        Some((first, rest)) => Ok(join(first, rest)),
    }
}

/// `ntpath.commonpath(paths)` (3.12 check order: abs/rel mix first, then drive).
/// Comparison is case-insensitive; the result keeps the case of `paths[0]`.
pub fn commonpath(paths: &[&str]) -> Result<String, PathError> {
    let Some(first) = paths.first() else {
        return Err(PathError::EmptySequence);
    };
    let lowered: Vec<String> = paths.iter().map(|p| p.replace('/', "\\").to_lowercase()).collect();
    let splits: Vec<(&str, &str, &str)> = lowered.iter().map(|p| splitroot(p)).collect();
    let roots: std::collections::BTreeSet<&str> = splits.iter().map(|s| s.1).collect();
    if roots.len() != 1 {
        return Err(PathError::MixAbsRel);
    }
    let drives: std::collections::BTreeSet<&str> = splits.iter().map(|s| s.0).collect();
    if drives.len() != 1 {
        return Err(PathError::DifferentDrives);
    }
    let first_norm = first.replace('/', "\\");
    let (drive, root, path) = splitroot(&first_norm);
    let common: Vec<&str> = path.split(SEP).filter(|c| !c.is_empty() && *c != ".").collect();
    let sp: Vec<Vec<&str>> =
        splits.iter().map(|s| s.2.split(SEP).filter(|c| !c.is_empty() && *c != ".").collect()).collect();
    let (Some(s1), Some(s2)) = (sp.iter().min(), sp.iter().max()) else {
        return Err(PathError::EmptySequence);
    };
    let n = s1.iter().zip(s2).take_while(|(a, b)| a == b).count();
    let keep = common.get(..n.min(common.len())).unwrap_or_default();
    Ok(format!("{drive}{root}{}", keep.join("\\")))
}

/// `ntpath.expanduser(p)` (3.12): `USERPROFILE` (even empty), else
/// `HOMEDRIVE`+`HOMEPATH`, else unchanged. `HOME` is ignored. No trailing
/// separator handling. `~user` is guessed as a sibling of the current profile.
pub fn expanduser(p: &str, env: impl Fn(&str) -> Option<String>) -> String {
    if !p.starts_with('~') {
        return p.to_owned();
    }
    let i = p.char_indices().skip(1).find(|&(_, c)| is_sep(c)).map_or(p.len(), |(i, _)| i);
    let mut userhome = if let Some(up) = env("USERPROFILE") {
        up
    } else if let Some(hp) = env("HOMEPATH") {
        join(&env("HOMEDRIVE").unwrap_or_default(), &[&hp])
    } else {
        return p.to_owned();
    };
    if i != 1 {
        let target = p.get(1..i).unwrap_or_default();
        let current = env("USERNAME");
        if current.as_deref() != Some(target) {
            if current.as_deref() != Some(basename(&userhome)) {
                return p.to_owned();
            }
            userhome = join(&dirname(&userhome), &[target]);
        }
    }
    format!("{userhome}{}", p.get(i..).unwrap_or_default())
}
