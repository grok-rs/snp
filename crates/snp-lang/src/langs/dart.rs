//! The `dart` language backend (§6.2; `pre_commit/languages/dart.py`).
//!
//! `install_environment` (`dart.py:38-97`) creates `<env>/bin` (it must not exist), then for the repository and for
//! every `additional_dependencies` package runs `_install_dir`: read `pubspec.yaml`, `dart pub get` in the package
//! directory with `PUB_CACHE` pointing to a temporary directory, and `dart compile exe --output <env>/bin/<name>
//! <pkg>/bin/<name>.dart` for every **key** of the pubspec's `executables` mapping (alias targets are ignored). A
//! dependency `name[:version]` is first fetched with `dart pub cache add name [--version version]` into its own
//! temporary `PUB_CACHE`; the first directory of that cache (top-down walk) holding a `pubspec.yaml` is copied to a
//! fresh temporary `pkg` directory and installed from there.
//!
//! snp differences: executable names must be single path components (a hostile pubspec cannot write outside
//! `<env>/bin`, 04b §5.7); the pubspec is read with a YAML subset reader ([`pubspec_executables`]) that understands the
//! shapes pubspecs use and refuses the rest with an error (exit 3, like pre-commit's `KeyError`/`TypeError` crashes);
//! the cache walk is sorted.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, copytree, default_version, io_err, listdir, mkdir_new, outcome,
    tempdir, win_exe,
};
use crate::common::{prefix_path, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: dart`.
#[derive(Debug)]
pub struct Dart;

/// Cap on a `pubspec.yaml` read (pubspecs are a few KiB).
const MAX_PUBSPEC: u64 = 1 << 20;

/// Depth bound of the `PUB_CACHE` walk.
const MAX_WALK_DEPTH: usize = 64;

/// What `pubspec['executables']` is, as far as `_install_dir` cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Executables {
    /// No `executables` key (pre-commit: `KeyError: 'executables'`).
    Missing,
    /// `executables:` with a null value (pre-commit: `TypeError: 'NoneType' object is not iterable`).
    Null,
    /// The names iterated by `for executable in pubspec['executables']`: the keys of a mapping (in document order),
    /// or the items of a sequence.
    Names(Vec<String>),
}

/// A `pubspec.yaml` the subset reader does not understand (or that is not a mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubspecError(pub String);

impl std::fmt::Display for PubspecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One logical line: indentation, content without comment.
#[derive(Debug, Clone, Copy)]
struct Line<'a> {
    indent: usize,
    text: &'a str,
}

/// The content of a YAML line without its comment (`#` at the start or after a blank, outside quotes), right-trimmed.
fn strip_comment(s: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev_blank = true;
    for (i, c) in s.char_indices() {
        if let Some(open) = quote {
            if c == open {
                quote = None;
            }
        } else {
            if c == '#' && prev_blank {
                return s.get(..i).unwrap_or(s).trim_end();
            }
            if (c == '\'' || c == '"') && prev_blank {
                quote = Some(c);
            }
        }
        prev_blank = c == ' ' || c == '\t';
    }
    s.trim_end()
}

/// Splits the document into content lines (BOM, directives, `---`/`...` markers, blank and comment lines dropped).
fn content_lines(text: &str) -> Result<Vec<Line<'_>>, PubspecError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    let mut started = false;
    for raw in text.split(['\n', '\r']) {
        if !started && raw.starts_with('%') {
            continue;
        }
        if raw == "---" || raw.starts_with("--- ") {
            if started && !out.is_empty() {
                return Err(PubspecError(
                    "expected a single document in pubspec.yaml".to_owned(),
                ));
            }
            started = true;
            let rest = strip_comment(raw.get(3..).unwrap_or_default()).trim();
            if !rest.is_empty() {
                return Err(PubspecError(
                    "unsupported pubspec.yaml: content on the document start line".to_owned(),
                ));
            }
            continue;
        }
        if raw == "..." || raw.starts_with("... ") {
            break;
        }
        started = true;
        let indent = raw.len().saturating_sub(raw.trim_start_matches(' ').len());
        let body = raw.get(indent..).unwrap_or_default();
        if body.starts_with('\t') && !strip_comment(body).trim().is_empty() {
            return Err(PubspecError(
                "found a tab character where an indentation space is expected".to_owned(),
            ));
        }
        let stripped = strip_comment(body);
        if stripped.trim().is_empty() {
            continue;
        }
        out.push(Line {
            indent,
            text: stripped,
        });
    }
    Ok(out)
}

/// Parses a quoted scalar at the start of `s`: (value, rest after the closing quote).
fn quoted(text: &str) -> Result<(String, &str), PubspecError> {
    let bad = || PubspecError(format!("unsupported quoted scalar in pubspec.yaml: {text}"));
    let mut chars = text.char_indices();
    let Some((_, delimiter)) = chars.next() else {
        return Err(bad());
    };
    let mut out = String::new();
    if delimiter == '\'' {
        let mut iter = chars.peekable();
        while let Some((pos, c)) = iter.next() {
            if c == '\'' {
                if iter.peek().is_some_and(|(_, next)| *next == '\'') {
                    iter.next();
                    out.push('\'');
                    continue;
                }
                return Ok((out, text.get(pos.saturating_add(1)..).unwrap_or_default()));
            }
            out.push(c);
        }
        return Err(bad());
    }
    while let Some((pos, c)) = chars.next() {
        match c {
            '"' => return Ok((out, text.get(pos.saturating_add(1)..).unwrap_or_default())),
            '\\' => {
                let Some((_, escape)) = chars.next() else {
                    return Err(bad());
                };
                match escape {
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '/' => out.push('/'),
                    ' ' => out.push(' '),
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '0' => out.push('\0'),
                    'x' | 'u' | 'U' => {
                        let width = match escape {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let mut code = 0_u32;
                        for _ in 0..width {
                            let Some((_, hex)) = chars.next() else {
                                return Err(bad());
                            };
                            let digit = hex.to_digit(16).ok_or_else(bad)?;
                            code = code
                                .checked_mul(16)
                                .and_then(|v| v.checked_add(digit))
                                .ok_or_else(bad)?;
                        }
                        out.push(char::from_u32(code).ok_or_else(bad)?);
                    }
                    _ => return Err(bad()),
                }
            }
            _ => out.push(c),
        }
    }
    Err(bad())
}

/// A mapping key at the start of `s` followed by `:` (end of line or blank after): (key, value text after the colon).
fn key_value(s: &str) -> Result<Option<(String, &str)>, PubspecError> {
    if s.starts_with('\'') || s.starts_with('"') {
        let (key, rest) = quoted(s)?;
        let rest = rest.trim_start();
        return Ok(rest
            .strip_prefix(':')
            .filter(|r| r.is_empty() || r.starts_with(' ') || r.starts_with('\t'))
            .map(|r| (key, r.trim())));
    }
    if s.starts_with('?') || s.starts_with('&') || s.starts_with('!') || s.starts_with('*') {
        return Err(PubspecError(format!(
            "unsupported mapping key in pubspec.yaml: {s}"
        )));
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b':' {
            let next = bytes.get(i.saturating_add(1));
            if next.is_none_or(|n| *n == b' ' || *n == b'\t') {
                let key = s.get(..i).unwrap_or_default().trim_end().to_owned();
                let value = s.get(i.saturating_add(1)..).unwrap_or_default().trim();
                return Ok(Some((key, value)));
            }
        }
    }
    Ok(None)
}

/// A plain or quoted scalar item (flow collection element or block sequence entry).
fn scalar(s: &str) -> Result<String, PubspecError> {
    let s = s.trim();
    if s.starts_with('\'') || s.starts_with('"') {
        let (v, rest) = quoted(s)?;
        if !rest.trim().is_empty() {
            return Err(PubspecError(format!(
                "unsupported scalar in pubspec.yaml: {s}"
            )));
        }
        return Ok(v);
    }
    if s.is_empty()
        || s.starts_with(['[', ']', '{', '}', '&', '*', '!', '|', '>', '@', '`', '?'])
        || s.starts_with("- ")
    {
        return Err(PubspecError(format!(
            "unsupported scalar in pubspec.yaml: {s}"
        )));
    }
    Ok(s.to_owned())
}

/// The elements of a one-line flow collection body (between the brackets), split at top-level commas.
fn flow_items(body: &str) -> Result<Vec<&str>, PubspecError> {
    let mut items = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0_usize;
    for (i, c) in body.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '[' | ']' | '{' | '}' => {
                    return Err(PubspecError(
                        "unsupported nested flow collection in pubspec.yaml".to_owned(),
                    ));
                }
                ',' => {
                    items.push(body.get(start..i).unwrap_or_default());
                    start = i.saturating_add(1);
                }
                _ => {}
            },
        }
    }
    items.push(body.get(start..).unwrap_or_default());
    // A trailing comma (`{a, }`) is allowed; empty elements elsewhere are not.
    if items.last().is_some_and(|l| l.trim().is_empty()) {
        items.pop();
    }
    if items.iter().any(|i| i.trim().is_empty()) {
        return Err(PubspecError(
            "empty flow collection entry in pubspec.yaml".to_owned(),
        ));
    }
    Ok(items)
}

/// The value of `executables:` written inline after the colon.
fn inline_value(v: &str, more: &[Line<'_>]) -> Result<Executables, PubspecError> {
    if matches!(v, "null" | "Null" | "NULL" | "~") {
        return Ok(Executables::Null);
    }
    let (open, close) = match v.chars().next() {
        Some('{') => ('{', '}'),
        Some('[') => ('[', ']'),
        _ => {
            return Err(PubspecError(format!(
                "unsupported `executables` value in pubspec.yaml: {v}"
            )));
        }
    };
    // A flow collection may continue on more-indented lines.
    let mut text = v.to_owned();
    for l in more {
        if text.ends_with(close) {
            break;
        }
        text.push(' ');
        text.push_str(l.text.trim());
    }
    let body = text
        .strip_prefix(open)
        .and_then(|t| t.strip_suffix(close))
        .ok_or_else(|| PubspecError("unterminated flow collection in pubspec.yaml".to_owned()))?;
    let mut names = Vec::new();
    for item in flow_items(body)? {
        let item = item.trim();
        if open == '{' {
            match key_value(item)? {
                Some((k, _)) => names.push(k),
                None => names.push(scalar(item)?),
            }
        } else {
            names.push(scalar(item)?);
        }
    }
    Ok(Executables::Names(names))
}

/// The block value of `executables:` (its more-indented lines, or a block sequence at the key's indentation).
fn block_value(children: &[Line<'_>]) -> Result<Executables, PubspecError> {
    let Some(first) = children.first() else {
        return Ok(Executables::Null);
    };
    let ci = first.indent;
    let is_seq = first.text == "-" || first.text.starts_with("- ");
    let mut names = Vec::new();
    for l in children {
        if l.indent < ci {
            return Err(PubspecError(
                "inconsistent indentation under `executables` in pubspec.yaml".to_owned(),
            ));
        }
        if l.indent > ci {
            // Nested value of a key (ignored, like pre-commit ignores alias targets).
            if is_seq {
                return Err(PubspecError(
                    "unsupported nested `executables` sequence entry in pubspec.yaml".to_owned(),
                ));
            }
            continue;
        }
        if is_seq {
            let item = l
                .text
                .strip_prefix('-')
                .filter(|r| r.is_empty() || r.starts_with(' '))
                .ok_or_else(|| {
                    PubspecError("mixed sequence and mapping under `executables`".to_owned())
                })?;
            if key_value(item.trim())?.is_some() {
                return Err(PubspecError(
                    "unsupported mapping inside the `executables` sequence".to_owned(),
                ));
            }
            names.push(scalar(item)?);
        } else {
            let (key, _value) = key_value(l.text)?.ok_or_else(|| {
                PubspecError(format!(
                    "expected `name:` under `executables` in pubspec.yaml, found: {}",
                    l.text
                ))
            })?;
            names.push(key);
        }
    }
    Ok(Executables::Names(names))
}

/// `yaml_load(pubspec.yaml)['executables']` as far as `_install_dir` iterates it, over the YAML subset pubspecs are
/// written in: a top-level block mapping (later duplicate keys win, as in PyYAML); `executables` as a block mapping
/// (`name:` / `name: target` lines, nested lines ignored), a block sequence, a one-line (or continued) flow mapping or
/// sequence (`{}`, `{a: b, c}`, `[a, b]`), or null. Plain, single- and double-quoted keys are supported; anchors, tags,
/// aliases, complex keys and block scalars are refused with an error.
///
/// Total on arbitrary input: every outcome is a value or a [`PubspecError`].
pub fn pubspec_executables(text: &str) -> Result<Executables, PubspecError> {
    let lines = content_lines(text)?;
    let mut result = Executables::Missing;
    let mut i = 0_usize;
    while let Some(line) = lines.get(i) {
        if line.indent != 0 {
            return Err(PubspecError(format!(
                "unexpected indentation in pubspec.yaml: {}",
                line.text
            )));
        }
        if line.text == "-" || line.text.starts_with("- ") || line.text.starts_with(['{', '[']) {
            return Err(PubspecError(
                "pubspec.yaml is not a block mapping".to_owned(),
            ));
        }
        let Some((key, value)) = key_value(line.text)? else {
            return Err(PubspecError(
                "pubspec.yaml is not a mapping of `key: value` lines".to_owned(),
            ));
        };
        // The lines that belong to this key: more indented, or (block sequence) `- ` at column 0.
        let mut j = i.saturating_add(1);
        while let Some(l) = lines.get(j) {
            let seq_at_zero = l.indent == 0 && (l.text == "-" || l.text.starts_with("- "));
            if l.indent == 0 && !seq_at_zero {
                break;
            }
            j = j.saturating_add(1);
        }
        let children = lines.get(i.saturating_add(1)..j).unwrap_or_default();
        if key == "executables" {
            result = if value.is_empty() {
                block_value(children)?
            } else {
                inline_value(value, children)?
            };
        }
        i = j;
    }
    Ok(result)
}

/// A name `dart compile exe` may write as `<env>/bin/<name>`: one path component (not empty, `.`, `..`, no separator,
/// no NUL; on Windows no `:` either).
pub fn valid_executable_name(name: &str) -> bool {
    !(name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
        || (cfg!(windows) && name.contains(['\\', ':'])))
}

/// The `ValueError` of an executable name that is not a single path component.
pub fn invalid_name_message(name: &str) -> String {
    format!(
        "pubspec.yaml executable {} is not a single path component",
        snp_pycompat::repr::repr_str(name)
    )
}

/// Reads `<pkg>/pubspec.yaml` (`open(…)` + `yaml_load`).
fn read_pubspec(pkg: &Path) -> Result<Executables, LangError> {
    use std::io::Read as _;
    let path = PathBuf::from(prefix_path(pkg, OsStr::new("pubspec.yaml")));
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|f| f.take(MAX_PUBSPEC).read_to_end(&mut bytes))
        .map_err(io_err("open", &path))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        LangError::Value(format!(
            "{}: pubspec.yaml is not valid UTF-8",
            path.display()
        ))
    })?;
    pubspec_executables(&text).map_err(|e| LangError::Value(format!("{}: {e}", path.display())))
}

/// `_install_dir(prefix_p, pub_cache)` (`dart.py:48-66`).
fn install_dir(
    cx: &InstallCtx<'_>,
    pkg: &Path,
    pub_cache: &Path,
    bin_dir: &Path,
) -> Result<(), LangError> {
    let host = cx.cx.host;
    let env = with_pub_cache(host.base_env, pub_cache);
    let executables = read_pubspec(pkg)?;
    let argv: Vec<OsString> = vec!["dart".into(), "pub".into(), "get".into()];
    setup_cmd(host.runner, &argv, &env, pkg)?;
    let names = match executables {
        Executables::Names(n) => n,
        Executables::Missing => {
            return Err(LangError::Value(format!(
                "{}: pubspec.yaml has no `executables` key",
                pkg.display()
            )));
        }
        Executables::Null => {
            return Err(LangError::Value(format!(
                "{}: pubspec.yaml `executables` is null",
                pkg.display()
            )));
        }
    };
    if let Some(bad) = names.iter().find(|n| !valid_executable_name(n)) {
        return Err(LangError::Value(invalid_name_message(bad)));
    }
    for name in names {
        let output = bin_dir.join(win_exe(&name));
        let source = prefix_path(pkg, OsStr::new(&format!("bin/{name}.dart")));
        let argv: Vec<OsString> = vec![
            "dart".into(),
            "compile".into(),
            "exe".into(),
            "--output".into(),
            output.into_os_string(),
            source,
        ];
        setup_cmd(host.runner, &argv, &env, pkg)?;
    }
    Ok(())
}

/// `{**os.environ, 'PUB_CACHE': pub_cache}`.
fn with_pub_cache(env: &Env, pub_cache: &Path) -> Env {
    env.patched(&EnvPatch::default().set("PUB_CACHE", pub_cache.as_os_str()))
}

/// `os.walk(top)` (top-down, not following directory symlinks, sorted): the first directory directly containing a
/// non-directory entry named `pubspec.yaml`.
fn find_pubspec_dir(top: &Path, depth: usize) -> Result<Option<PathBuf>, LangError> {
    if depth > MAX_WALK_DEPTH {
        return Ok(None);
    }
    let names = listdir(top)?;
    let mut subdirs = Vec::new();
    let mut found = false;
    for name in names {
        let p = top.join(&name);
        let is_dir = std::fs::metadata(&p).is_ok_and(|m| m.is_dir());
        if is_dir {
            let is_link = std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
            if !is_link {
                subdirs.push(p);
            }
        } else if name == OsStr::new("pubspec.yaml") {
            found = true;
        }
    }
    if found {
        return Ok(Some(top.to_path_buf()));
    }
    for d in subdirs {
        if let Some(hit) = find_pubspec_dir(&d, depth.saturating_add(1))? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

impl Language for Dart {
    fn id(&self) -> LanguageId {
        LanguageId::Dart
    }

    /// `assert_version_default('dart', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Dart)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`dart.py:38-97`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Dart, cx)?;
        let host = cx.cx.host;
        let bin_dir = cx.cx.env_dir.join("bin");
        mkdir_new(&bin_dir)?;
        {
            let tmp = tempdir(host.base_env)?;
            install_dir(cx, cx.cx.prefix, tmp.path(), &bin_dir)?;
        }
        for dep_s in cx.deps {
            let dep_tmp = tempdir(host.base_env)?;
            let (dep, _, version) = snp_pycompat::text::partition(dep_s, ":");
            let mut argv: Vec<OsString> = vec![
                "dart".into(),
                "pub".into(),
                "cache".into(),
                "add".into(),
                dep.into(),
            ];
            if !version.is_empty() {
                argv.push("--version".into());
                argv.push(version.into());
            }
            let env = with_pub_cache(host.base_env, dep_tmp.path());
            setup_cmd(host.runner, &argv, &env, cx.cx.prefix)?;
            let Some(root) = find_pubspec_dir(dep_tmp.path(), 0)? else {
                return Err(LangError::Assertion(format!(
                    "could not find pubspec.yaml for {dep_s}"
                )));
            };
            let copied = tempdir(host.base_env)?;
            let pkg = copied.path().join("pkg");
            copytree(&root, &pkg)?;
            install_dir(cx, &pkg, dep_tmp.path(), &bin_dir)?;
        }
        Ok(outcome("dart"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)`: `<env>/bin` first on `PATH` (no `PUB_CACHE` at run time).
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(EnvPatch::default().prepend_path("PATH", [cx.env_dir.join("bin")]))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
