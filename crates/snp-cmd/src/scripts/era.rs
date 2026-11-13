//! Argument recovery from other managers' hook scripts (11 §A.5.3): `doctor --fix` keeps a replaced script's
//! options, and M1 names the Python interpreter a pre-commit script used.
//!
//! Every parser is total and bounded: it never panics, never allocates more than a small multiple of its input,
//! and answers `None` when a script's shape is not understood (the caller then uses the defaults and adds M1's
//! ` (could not read its options; defaults used)` note). Fuzzed by `cmd_script_args`.
//!
//! Owner: WP-33.

use snp_git::hookscripts::Owner;

/// What a hook script tells about how it was installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptArgs {
    /// pre-commit's `INSTALL_PYTHON` (bytes as written, unquoted).
    pub install_python: Option<Vec<u8>>,
    /// The `--config=` value (`CONFIG` in era 1), unquoted.
    pub config: Option<Vec<u8>>,
    /// `--skip-on-missing-config` (`SKIP_ON_MISSING_CONFIG = True`, snp 1.x `--allow-missing-config`).
    pub skip_on_missing_config: bool,
    /// An snp script's `SNP_EXE`, unquoted.
    pub snp_exe: Option<Vec<u8>>,
    /// prek-only options (`--cd=…`, selectors): such a script cannot be adopted (M1b).
    pub prek_only: Vec<String>,
}

/// Recovers the installation options of a hook script whose owner is `owner` (from `classify`).
///
/// | owner | fields |
/// |---|---|
/// | pre-commit era 3 (bash) | `INSTALL_PYTHON=<sh word>`, `ARGS=(<sh words>)` |
/// | pre-commit era 2 (Python) | `INSTALL_PYTHON = '<repr>'`, `ARGS = [<repr>, …]` |
/// | pre-commit era 1 (Python) | `CONFIG = '…'`, `INSTALL_PYTHON = '…'`, `SKIP_ON_MISSING_CONFIG = True\|False` |
/// | prek | the `hook-impl` line: `--config="…"`, `--skip-on-missing-config`, anything else prek-only |
/// | snp 1.x | `--config="…"`, `--allow-missing-config` |
/// | snp | `SNP_EXE=<sh word>` and the `set --` line |
///
/// `None` = unparseable (or a pre-2018 pre-commit template, or a foreign script): use the defaults.
pub fn parse_script_args(owner: &Owner, bytes: &[u8]) -> Option<ScriptArgs> {
    match owner {
        Owner::PreCommit { era: Some(3) } => era3(bytes),
        Owner::PreCommit { era: Some(2) } => era2(bytes),
        Owner::PreCommit { era: Some(1) } => era1(bytes),
        Owner::Prek => prek(bytes),
        Owner::SnpV1 => snp_v1(bytes),
        Owner::Snp { .. } => snp(bytes),
        _ => None,
    }
}

/// The lines of `b` (`\n`-split, a trailing `\r` dropped), each bounded by the input.
fn lines(b: &[u8]) -> impl Iterator<Item = &[u8]> {
    b.split(|&c| c == b'\n').map(|l| l.trim_suffix(b"\r"))
}

fn trim(b: &[u8]) -> &[u8] {
    b.trim_ascii()
}

/// Applies the `hook-impl` options of pre-commit's `ARGS` to `out`.
fn apply_args(out: &mut ScriptArgs, words: &[Vec<u8>]) {
    let mut it = words.iter();
    while let Some(w) = it.next() {
        if let Some(v) = w.strip_prefix(b"--config=") {
            out.config = Some(v.to_vec());
        } else if w == b"--config" || w == b"-c" {
            out.config = it.next().cloned();
        } else if w == b"--skip-on-missing-config" || w == b"--allow-missing-config" {
            out.skip_on_missing_config = true;
        }
    }
}

/// pre-commit ≥ 2021-09: `INSTALL_PYTHON=<shlex.quote>` and `ARGS=(<shlex.join>)`.
fn era3(b: &[u8]) -> Option<ScriptArgs> {
    let mut out = ScriptArgs::default();
    let mut saw_args = false;
    for line in lines(b) {
        if let Some(rest) = line.strip_prefix(b"INSTALL_PYTHON=") {
            let words = sh_words(rest)?;
            out.install_python = Some(words.into_iter().next().unwrap_or_default());
        } else if let Some(rest) = line.strip_prefix(b"ARGS=(") {
            let inner = trim(rest).strip_suffix(b")")?;
            apply_args(&mut out, &sh_words(inner)?);
            saw_args = true;
        }
    }
    saw_args.then_some(out)
}

/// pre-commit 2020-01 → 2021-09: `INSTALL_PYTHON = '<repr>'` and `ARGS = ['hook-impl', …]`.
fn era2(b: &[u8]) -> Option<ScriptArgs> {
    let mut out = ScriptArgs::default();
    let mut saw_args = false;
    for line in lines(b) {
        if let Some(rest) = line.strip_prefix(b"INSTALL_PYTHON = ") {
            out.install_python = Some(py_str(trim(rest))?.0);
        } else if let Some(rest) = line.strip_prefix(b"ARGS = ") {
            apply_args(&mut out, &py_list(trim(rest))?);
            saw_args = true;
        }
    }
    saw_args.then_some(out)
}

/// pre-commit 2018-02 → 2020-01: `CONFIG = '…'`, `INSTALL_PYTHON = '…'`, `SKIP_ON_MISSING_CONFIG = True`.
fn era1(b: &[u8]) -> Option<ScriptArgs> {
    let mut out = ScriptArgs::default();
    let mut saw_config = false;
    for line in lines(b) {
        if let Some(rest) = line.strip_prefix(b"CONFIG = ") {
            out.config = Some(py_str(trim(rest))?.0);
            saw_config = true;
        } else if let Some(rest) = line.strip_prefix(b"INSTALL_PYTHON = ") {
            out.install_python = Some(py_str(trim(rest))?.0);
        } else if let Some(rest) = line.strip_prefix(b"SKIP_ON_MISSING_CONFIG = ") {
            out.skip_on_missing_config = match trim(rest) {
                b"True" => true,
                b"False" => false,
                _ => return None,
            };
        }
    }
    saw_config.then_some(out)
}

/// prek: `exec "$PREK" hook-impl --hook-dir "$HERE" --script-version 4 [ARGS] -- "$@"`.
fn prek(b: &[u8]) -> Option<ScriptArgs> {
    let line = lines(b).find(|l| trim(l).starts_with(b"exec ") && contains(l, b"hook-impl"))?;
    let words = sh_words(line)?;
    let start = words.iter().position(|w| w == b"hook-impl")?;
    let mut out = ScriptArgs::default();
    let mut it = words.iter().skip(start.saturating_add(1));
    while let Some(w) = it.next() {
        if w == b"--" {
            break;
        }
        if w == b"--hook-dir" || w == b"--script-version" {
            let _value = it.next();
        } else if let Some(v) = w.strip_prefix(b"--config=") {
            out.config = Some(v.to_vec());
        } else if w == b"--skip-on-missing-config" {
            out.skip_on_missing_config = true;
        } else if w.starts_with(b"--hook-type=") {
        } else {
            out.prek_only.push(String::from_utf8_lossy(w).into_owned());
        }
    }
    Some(out)
}

/// snp 1.x: a `--config="…"` before `run` and `--allow-missing-config`, on any line of the script.
fn snp_v1(b: &[u8]) -> Option<ScriptArgs> {
    let mut out = ScriptArgs::default();
    let mut saw = false;
    for line in lines(b) {
        if !contains(line, b"--config") && !contains(line, b"--allow-missing-config") {
            continue;
        }
        let Some(words) = sh_words(line) else {
            continue;
        };
        apply_args(&mut out, &words);
        saw = true;
    }
    saw.then_some(out)
}

/// snp's own script: `SNP_EXE=<word>` and the `set -- hook-impl …` line. Both are parsed as shell statements, not
/// lines: `render` quotes with `shlex.quote`, so a config path or `SNP_EXE` holding a newline spans lines inside its
/// quotes.
fn snp(b: &[u8]) -> Option<ScriptArgs> {
    let mut out = ScriptArgs::default();
    let mut saw = false;
    let mut pos = 0_usize;
    while let Some(rest) = b.get(pos..).filter(|r| !r.is_empty()) {
        let next_line = rest
            .iter()
            .position(|&c| c == b'\n')
            .map_or(rest.len(), |n| n.saturating_add(1));
        let step = if let Some(stmt) = rest.strip_prefix(b"SNP_EXE=") {
            let (words, used) = sh_split(stmt, true)?;
            out.snp_exe = Some(words.into_iter().next().unwrap_or_default());
            used.saturating_add(8)
        } else if let Some(stmt) = rest.strip_prefix(b"set -- ") {
            let (words, used) = sh_split(stmt, true)?;
            apply_args(&mut out, &words);
            saw = true;
            used.saturating_add(7)
        } else {
            next_line
        };
        pos = pos.saturating_add(step.max(1));
    }
    saw.then_some(out)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}

/// POSIX `sh` word splitting of one line, without expansion: blanks separate words; `'…'` is literal; `"…"` keeps
/// `\` only before `$`, `` ` ``, `"`, `\`; an unquoted `\` escapes the next byte; an unquoted `#` at a word start
/// starts a comment. `$NAME` / `"$@"` stay literal text. `None` for an unterminated quote.
pub(crate) fn sh_words(line: &[u8]) -> Option<Vec<Vec<u8>>> {
    sh_split(line, false).map(|(w, _)| w)
}

/// [`sh_words`] over `line`; with `statement`, an unquoted LF ends the statement (a CR before it is a blank) and a
/// quoted one is literal. Returns the words and the bytes consumed (through the terminating LF).
fn sh_split(line: &[u8], statement: bool) -> Option<(Vec<Vec<u8>>, usize)> {
    let mut words: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut in_word = false;
    let mut i = 0_usize;
    while let Some(&c) = line.get(i) {
        i = i.saturating_add(1);
        match c {
            b'\n' if statement => break,
            b' ' | b'\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            b'\r' if statement && line.get(i) == Some(&b'\n') => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            b'#' if !in_word => {
                if statement {
                    let skip = line.get(i..)?.iter().position(|&b| b == b'\n');
                    i = skip.map_or(line.len(), |n| i.saturating_add(n).saturating_add(1));
                }
                break;
            }
            b'\'' => {
                in_word = true;
                let end = line.get(i..)?.iter().position(|&b| b == b'\'')?;
                cur.extend_from_slice(line.get(i..i.saturating_add(end))?);
                i = i.saturating_add(end).saturating_add(1);
            }
            b'"' => {
                in_word = true;
                loop {
                    let &d = line.get(i)?;
                    i = i.saturating_add(1);
                    match d {
                        b'"' => break,
                        b'\\' => match line.get(i) {
                            Some(&e @ (b'$' | b'`' | b'"' | b'\\')) => {
                                cur.push(e);
                                i = i.saturating_add(1);
                            }
                            Some(_) => cur.push(b'\\'),
                            None => return None,
                        },
                        _ => cur.push(d),
                    }
                }
            }
            b'\\' => {
                in_word = true;
                if let Some(&e) = line.get(i) {
                    cur.push(e);
                    i = i.saturating_add(1);
                }
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Some((words, i))
}

/// A Python string literal at the start of `b` (single or double quotes; `\\`, `\'`, `\"`, `\n`, `\t`, `\r`,
/// `\0`, `\xNN`, `\uNNNN`, `\UNNNNNNNN`): its UTF-8 bytes and the remaining input.
pub(crate) fn py_str(b: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let (&q, mut rest) = b.split_first()?;
    if q != b'\'' && q != b'"' {
        return None;
    }
    let mut out = Vec::new();
    loop {
        let (&c, tail) = rest.split_first()?;
        rest = tail;
        if c == q {
            return Some((out, rest));
        }
        if c == b'\n' {
            return None;
        }
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let (&e, tail) = rest.split_first()?;
        rest = tail;
        let simple = match e {
            b'\\' => Some(b'\\'),
            b'\'' => Some(b'\''),
            b'"' => Some(b'"'),
            b'n' => Some(b'\n'),
            b't' => Some(b'\t'),
            b'r' => Some(b'\r'),
            b'0' => Some(0),
            _ => None,
        };
        if let Some(s) = simple {
            out.push(s);
            continue;
        }
        let digits = match e {
            b'x' => 2,
            b'u' => 4,
            b'U' => 8,
            _ => {
                // an unknown escape keeps its backslash (Python's behaviour, with a DeprecationWarning)
                out.push(b'\\');
                out.push(e);
                continue;
            }
        };
        let hex = rest.get(..digits)?;
        rest = rest.get(digits..)?;
        let code = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
        let ch = char::from_u32(code)?;
        let mut buf = [0_u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

/// A Python list of string literals: `[<lit>, <lit>, …]` (a trailing comma allowed).
pub(crate) fn py_list(b: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut rest = trim(b).strip_prefix(b"[")?;
    let mut out = Vec::new();
    loop {
        rest = rest.trim_ascii_start();
        if let Some(r) = rest.strip_prefix(b"]") {
            return trim(r).is_empty().then_some(out);
        }
        let (s, r) = py_str(rest)?;
        out.push(s);
        rest = r.trim_ascii_start();
        if let Some(r) = rest.strip_prefix(b",") {
            rest = r;
        } else if !rest.starts_with(b"]") {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<Vec<u8>> {
        sh_words(s.as_bytes()).unwrap()
    }

    #[test]
    fn sh_words_quoting() {
        assert_eq!(w("a b  c"), [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
        assert_eq!(
            w("'--config=my cfg'\"'\"'s.yaml' x"),
            [b"--config=my cfg's.yaml".to_vec(), b"x".to_vec()]
        );
        assert_eq!(w("\"a\\\"b\" c\\ d"), [b"a\"b".to_vec(), b"c d".to_vec()]);
        assert_eq!(w("x # y"), [b"x".to_vec()]);
        assert_eq!(w("a#b"), [b"a#b".to_vec()]);
        assert_eq!(w("''"), [Vec::<u8>::new()]);
        assert!(sh_words(b"'open").is_none());
        assert!(sh_words(b"\"open").is_none());
        assert_eq!(w("\"$HERE\""), [b"$HERE".to_vec()]);
    }

    #[test]
    fn py_literals() {
        assert_eq!(
            py_str(b"'a\\'b' rest").unwrap(),
            (b"a'b".to_vec(), &b" rest"[..])
        );
        assert_eq!(py_str(b"\"\\x41\\u00e9\"").unwrap().0, "Aé".as_bytes());
        assert!(py_str(b"'open").is_none());
        assert!(py_str(b"'\\x4'").is_none());
        assert_eq!(
            py_list(b"['hook-impl', '--config=x.yaml', ]").unwrap(),
            [b"hook-impl".to_vec(), b"--config=x.yaml".to_vec()]
        );
        assert!(py_list(b"['a' 'b']").is_none());
        assert!(py_list(b"['a'] x").is_none());
    }

    #[test]
    fn era3_script() {
        let s = b"#!/usr/bin/env bash\n# ID: 138fd403232d2ddd5efb44317e38bf03\n# start templated\n\
INSTALL_PYTHON=/home/u/.venv/bin/python\nARGS=(hook-impl '--config=my cfg.yaml' --hook-type=pre-commit --skip-on-missing-config)\n# end templated\n";
        let a = parse_script_args(&Owner::PreCommit { era: Some(3) }, s).unwrap();
        assert_eq!(
            a.install_python.as_deref(),
            Some(&b"/home/u/.venv/bin/python"[..])
        );
        assert_eq!(a.config.as_deref(), Some(&b"my cfg.yaml"[..]));
        assert!(a.skip_on_missing_config);
    }

    #[test]
    fn era2_and_era1_scripts() {
        let s = b"INSTALL_PYTHON = '/usr/bin/python3'\nARGS = ['hook-impl', '--config=.pre-commit-config.yaml', '--hook-type=pre-push']\n";
        let a = parse_script_args(&Owner::PreCommit { era: Some(2) }, s).unwrap();
        assert_eq!(a.install_python.as_deref(), Some(&b"/usr/bin/python3"[..]));
        assert_eq!(a.config.as_deref(), Some(&b".pre-commit-config.yaml"[..]));
        assert!(!a.skip_on_missing_config);
        let s = b"CONFIG = 'c.yaml'\nHOOK_TYPE = 'pre-commit'\nINSTALL_PYTHON = '/p'\nSKIP_ON_MISSING_CONFIG = True\n";
        let a = parse_script_args(&Owner::PreCommit { era: Some(1) }, s).unwrap();
        assert_eq!(a.config.as_deref(), Some(&b"c.yaml"[..]));
        assert!(a.skip_on_missing_config);
        assert!(parse_script_args(&Owner::PreCommit { era: None }, s).is_none());
        assert!(parse_script_args(&Owner::Foreign, s).is_none());
    }

    #[test]
    fn prek_script() {
        let s = b"#!/bin/sh\n# ID: 182c10f181da4464a3eec51b83331688\nHERE=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
PREK=\"/usr/bin/prek\"\nexec \"$PREK\" hook-impl --hook-dir \"$HERE\" --script-version 4 --hook-type=pre-commit --config=\"c.yaml\" --cd=\"sub\" -- \"$@\"\n";
        let a = parse_script_args(&Owner::Prek, s).unwrap();
        assert_eq!(a.config.as_deref(), Some(&b"c.yaml"[..]));
        assert_eq!(a.prek_only, ["--cd=sub"]);
    }

    #[test]
    fn snp_scripts() {
        let s = crate::scripts::render(
            std::ffi::OsStr::new("x y.yaml"),
            snp_core::stage::HookType::PreCommit,
            true,
            std::path::Path::new("/opt/bin/snp"),
        );
        let a = parse_script_args(&Owner::Snp { stale: false }, &s).unwrap();
        assert_eq!(a.snp_exe.as_deref(), Some(&b"/opt/bin/snp"[..]));
        assert_eq!(a.config.as_deref(), Some(&b"x y.yaml"[..]));
        assert!(a.skip_on_missing_config);
        let v1 = b"#!/bin/sh\n# Generated by SNP (Shell Not Pass)\nexec snp --config=\".pre-commit-config.yaml\" run --hook-stage pre-commit --allow-missing-config \"$@\"\n";
        let a = parse_script_args(&Owner::SnpV1, v1).unwrap();
        assert_eq!(a.config.as_deref(), Some(&b".pre-commit-config.yaml"[..]));
        assert!(a.skip_on_missing_config);
    }
}
