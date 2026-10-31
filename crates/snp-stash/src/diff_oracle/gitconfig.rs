//! The few git-config facts the `DiffOracle` needs without spawning git (ADR-078): `include.path` /
//! `includeIf.<cond>.path` targets (watched even when they do not exist yet), `core.attributesFile` and whether
//! `attr.tree` is set.
//!
//! A deliberately small reader of git's config syntax (`config.c`): `[section]`, `[section "sub"]`,
//! `[section.sub]`, `name = value` with quotes, backslash escapes, `#`/`;` comments and line continuations. It only
//! has to find these three keys; a file it misreads costs at most a spurious "changed" (the real `git diff` then
//! decides), except a missed include target — so every `path` key of an `include*` section is taken, whatever its
//! condition.
//!
//! Owner: WP-23.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use snp_core::env::Env;

/// One `key = value` entry: `(section, key, value)`, section and key lower-cased (subsections dropped).
fn entries(text: &[u8]) -> Vec<(String, String, Option<Vec<u8>>)> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut i = 0_usize;
    let n = text.len();
    let at = |k: usize| text.get(k).copied();
    while i < n {
        // Skip whitespace and blank lines.
        while matches!(at(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            i = i.saturating_add(1);
        }
        match at(i) {
            None => break,
            Some(b'#' | b';') => {
                while !matches!(at(i), None | Some(b'\n')) {
                    i = i.saturating_add(1);
                }
            }
            Some(b'[') => {
                i = i.saturating_add(1);
                let mut name = String::new();
                while let Some(c) = at(i) {
                    if c == b']' || c == b'"' || c == b'\n' || c == b' ' || c == b'\t' || c == b'.'
                    {
                        break;
                    }
                    name.push(char::from(c.to_ascii_lowercase()));
                    i = i.saturating_add(1);
                }
                // Skip the subsection (quoted, with escapes) up to `]`.
                let mut in_quote = false;
                while let Some(c) = at(i) {
                    match c {
                        b'\\' if in_quote => i = i.saturating_add(1),
                        b'"' => in_quote = !in_quote,
                        b']' if !in_quote => break,
                        b'\n' => break,
                        _ => {}
                    }
                    i = i.saturating_add(1);
                }
                if at(i) == Some(b']') {
                    i = i.saturating_add(1);
                }
                section = name;
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let mut key = String::new();
                while let Some(c) = at(i) {
                    if c.is_ascii_alphanumeric() || c == b'-' {
                        key.push(char::from(c.to_ascii_lowercase()));
                        i = i.saturating_add(1);
                    } else {
                        break;
                    }
                }
                while matches!(at(i), Some(b' ' | b'\t')) {
                    i = i.saturating_add(1);
                }
                if at(i) == Some(b'=') {
                    i = i.saturating_add(1);
                    let (value, next) = value(text, i);
                    i = next;
                    out.push((section.clone(), key, Some(value)));
                } else {
                    out.push((section.clone(), key, None));
                    while !matches!(at(i), None | Some(b'\n')) {
                        i = i.saturating_add(1);
                    }
                }
            }
            Some(_) => {
                // Not something git accepts: skip the line.
                while !matches!(at(i), None | Some(b'\n')) {
                    i = i.saturating_add(1);
                }
            }
        }
    }
    out
}

/// `parse_value` of `config.c`: returns the value and the index after its line.
fn value(text: &[u8], mut i: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut in_quote = false;
    // Trailing unquoted whitespace is dropped; inner whitespace runs are kept.
    let mut pending_space = 0_usize;
    let at = |k: usize| text.get(k).copied();
    while matches!(at(i), Some(b' ' | b'\t')) {
        i = i.saturating_add(1);
    }
    while let Some(c) = at(i) {
        i = i.saturating_add(1);
        match c {
            b'\n' => break,
            b'\r' if at(i) == Some(b'\n') => {}
            b'#' | b';' if !in_quote => {
                while !matches!(at(i), None | Some(b'\n')) {
                    i = i.saturating_add(1);
                }
            }
            b' ' | b'\t' if !in_quote => pending_space = pending_space.saturating_add(1),
            b'"' => in_quote = !in_quote,
            b'\\' => {
                let esc = at(i);
                i = i.saturating_add(1);
                let ch = match esc {
                    Some(b'\n') => continue,
                    Some(b'\r') if at(i) == Some(b'\n') => {
                        i = i.saturating_add(1);
                        continue;
                    }
                    Some(b'n') => b'\n',
                    Some(b't') => b'\t',
                    Some(b'b') => 0x08,
                    Some(other) => other,
                    None => break,
                };
                out.extend(std::iter::repeat_n(b' ', pending_space));
                pending_space = 0;
                out.push(ch);
            }
            _ => {
                out.extend(std::iter::repeat_n(b' ', pending_space));
                pending_space = 0;
                out.push(c);
            }
        }
    }
    (out, i)
}

/// What the oracle learns from the config files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Facts {
    /// Every config file read or watched (origins, the standard locations and include targets).
    pub(crate) config_files: Vec<PathBuf>,
    /// `core.attributesFile` targets (every value seen: the last one wins in git, watching all is conservative).
    pub(crate) attributes_files: Vec<PathBuf>,
    /// `attr.tree` is set (Stat mode impossible).
    pub(crate) attr_tree: bool,
    /// More include targets (or `core.attributesFile` values) than [`MAX_FILES`]: the inputs are not known
    /// completely (Stat mode impossible).
    pub(crate) overflow: bool,
}

/// Expands a config path value (`~/`, `~user/`; relative to `base` when relative).
fn config_path(raw: &[u8], base: &Path, env: &Env) -> Option<PathBuf> {
    if raw.is_empty() {
        return None;
    }
    let p = crate::journal::bytes_path(raw);
    let expanded = if raw.first() == Some(&b'~') {
        PathBuf::from(snp_os::home::expanduser(p.as_os_str(), env))
    } else {
        p
    };
    Some(if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    })
}

/// The largest config file read.
const CONFIG_CAP: u64 = 4 * 1024 * 1024;
/// At most this many config files are watched (seeds and include targets together) and this many
/// `core.attributesFile` values kept; more means Git mode (include loops, hostile configs).
const MAX_FILES: usize = 256;

/// Reads `seeds` (and, transitively, every include target) and collects the [`Facts`]. `root` resolves a relative
/// `core.attributesFile`.
///
/// Linear in the total config size: queued paths are deduplicated with a hash set, and more than [`MAX_FILES`]
/// distinct files (or attributes files) stops the walk with [`Facts::overflow`] set, so an input it never read can
/// never be missed.
pub(crate) fn collect(seeds: &[PathBuf], root: &Path, env: &Env) -> Facts {
    let mut facts = Facts::default();
    let mut queue: Vec<PathBuf> = Vec::new();
    let mut queued: HashSet<PathBuf> = HashSet::new();
    let mut attrs: HashSet<PathBuf> = HashSet::new();
    let mut enqueue = |p: PathBuf, queue: &mut Vec<PathBuf>, facts: &mut Facts| {
        if queued.contains(&p) {
            return true;
        }
        if queue.len() >= MAX_FILES {
            facts.overflow = true;
            return false;
        }
        queued.insert(p.clone());
        queue.push(p);
        true
    };
    for s in seeds {
        if !enqueue(s.clone(), &mut queue, &mut facts) {
            break;
        }
    }
    let mut next = 0_usize;
    'files: while !facts.overflow
        && let Some(file) = queue.get(next).cloned()
    {
        next = next.saturating_add(1);
        let Ok(text) = snp_os::fs::read_capped(&file, CONFIG_CAP) else {
            continue;
        };
        let base = file
            .parent()
            .map_or_else(|| root.to_path_buf(), Path::to_path_buf);
        for (section, key, value) in entries(&text) {
            match (section.as_str(), key.as_str(), value) {
                ("include" | "includeif", "path", Some(v)) => {
                    if let Some(p) = config_path(&v, &base, env)
                        && !enqueue(p, &mut queue, &mut facts)
                    {
                        break 'files;
                    }
                }
                ("core", "attributesfile", Some(v)) => {
                    if let Some(p) = config_path(&v, root, env)
                        && !attrs.contains(&p)
                    {
                        if attrs.len() >= MAX_FILES {
                            facts.overflow = true;
                            break 'files;
                        }
                        attrs.insert(p.clone());
                        facts.attributes_files.push(p);
                    }
                }
                ("attr", "tree", _) => facts.attr_tree = true,
                _ => {}
            }
        }
    }
    facts.config_files = queue;
    facts
}

/// `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>` and `GIT_CONFIG_PARAMETERS`: the same three facts
/// from the environment. `None` when `GIT_CONFIG_PARAMETERS` mentions one of them (its quoting is not worth
/// emulating: Git mode).
pub(crate) fn from_env(env: &Env, root: &Path) -> Option<Facts> {
    let mut facts = Facts::default();
    if let Some(params) = env.get("GIT_CONFIG_PARAMETERS") {
        let lower = params.to_string_lossy().to_ascii_lowercase();
        if lower.contains("include")
            || lower.contains("attributesfile")
            || lower.contains("attr.tree")
        {
            return None;
        }
    }
    let count = env
        .get("GIT_CONFIG_COUNT")
        .and_then(|v| v.to_str())
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0)
        .min(10_000);
    for n in 0..count {
        let Some(key) = env.get(&format!("GIT_CONFIG_KEY_{n}")) else {
            continue;
        };
        let key = key.to_string_lossy().to_ascii_lowercase();
        let value = env
            .get(&format!("GIT_CONFIG_VALUE_{n}"))
            .map(|v| crate::journal::path_bytes(Path::new(v)).into_owned())
            .unwrap_or_default();
        let is_include = key == "include.path"
            || key
                .strip_prefix("includeif.")
                .is_some_and(|rest| rest.rsplit_once('.').is_some_and(|(_, k)| k == "path"));
        if is_include {
            if let Some(p) = config_path(&value, root, env) {
                facts.config_files.push(p);
            }
        } else if key == "core.attributesfile" {
            if let Some(p) = config_path(&value, root, env) {
                facts.attributes_files.push(p);
            }
        } else if key == "attr.tree" {
            facts.attr_tree = true;
        }
    }
    Some(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<(String, String, Option<String>)> {
        entries(text.as_bytes())
            .into_iter()
            .map(|(s, k, v)| (s, k, v.map(|v| String::from_utf8(v).unwrap())))
            .collect()
    }

    #[test]
    fn parses_git_config_syntax() {
        let got = keys(
            "# c\n[core]\n\tAttributesFile = ~/attrs ; comment\n\tbare\n[include]\n  path = \"a b.cfg\"\n\
             [includeIf \"gitdir:~/x/\"]\n\tpath = x\\\n.cfg\n[attr] tree = HEAD\n[Section.Sub]\nk=v\\tw  \n",
        );
        assert_eq!(
            got,
            vec![
                (
                    "core".into(),
                    "attributesfile".into(),
                    Some("~/attrs".into())
                ),
                ("core".into(), "bare".into(), None),
                ("include".into(), "path".into(), Some("a b.cfg".into())),
                ("includeif".into(), "path".into(), Some("x.cfg".into())),
                ("attr".into(), "tree".into(), Some("HEAD".into())),
                ("section".into(), "k".into(), Some("v\tw".into())),
            ]
        );
    }

    #[test]
    fn hostile_input_terminates() {
        for t in [
            &b"["[..],
            b"[\"",
            b"a=\"",
            b"a=\\",
            b"\xff\xfe=\x00",
            b"[x \"\\",
        ] {
            drop(entries(t));
        }
    }

    #[test]
    fn collect_follows_includes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(
            root.join("a.cfg"),
            "[include]\npath = b.cfg\n[includeIf \"onbranch:x\"]\npath = missing.cfg\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.cfg"),
            "[core]\nattributesFile = attrs\n[include]\npath = a.cfg\n",
        )
        .unwrap();
        let env = Env::from_pairs([("HOME", "/h")]);
        let f = collect(&[root.join("a.cfg")], root, &env);
        assert_eq!(
            f.config_files,
            vec![
                root.join("a.cfg"),
                root.join("b.cfg"),
                root.join("missing.cfg")
            ]
        );
        assert_eq!(f.attributes_files, vec![root.join("attrs")]);
        assert!(!f.attr_tree);
    }

    #[test]
    fn a_huge_include_list_returns_quickly_and_overflows() {
        use std::fmt::Write as _;
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mut cfg = String::from("[include]\n");
        for i in 0..30_000 {
            writeln!(cfg, "path = missing{i}").unwrap();
        }
        std::fs::write(root.join("config"), cfg).unwrap();
        let env = Env::from_pairs([("HOME", "/h")]);
        let t = std::time::Instant::now();
        let f = collect(&[root.join("config")], root, &env);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
        assert!(f.overflow);
        assert!(f.config_files.len() <= MAX_FILES);
    }

    #[test]
    fn a_long_include_chain_overflows() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for i in 0..300 {
            std::fs::write(
                root.join(format!("c{i}.cfg")),
                format!("[include]\npath = c{}.cfg\n", i + 1),
            )
            .unwrap();
        }
        let env = Env::from_pairs([("HOME", "/h")]);
        let f = collect(&[root.join("c0.cfg")], root, &env);
        assert!(f.overflow);
        // A chain within the cap is followed completely.
        let f = collect(&[root.join("c200.cfg")], root, &env);
        assert!(!f.overflow);
        assert_eq!(f.config_files.len(), 101);
    }

    #[test]
    fn env_facts() {
        let env = Env::from_pairs([
            ("GIT_CONFIG_COUNT", "3"),
            ("GIT_CONFIG_KEY_0", "include.path"),
            ("GIT_CONFIG_VALUE_0", "/x.cfg"),
            ("GIT_CONFIG_KEY_1", "core.attributesFile"),
            ("GIT_CONFIG_VALUE_1", "/attrs"),
            ("GIT_CONFIG_KEY_2", "attr.tree"),
            ("GIT_CONFIG_VALUE_2", "HEAD"),
        ]);
        let f = from_env(&env, Path::new("/r")).unwrap();
        assert_eq!(f.config_files, vec![PathBuf::from("/x.cfg")]);
        assert_eq!(f.attributes_files, vec![PathBuf::from("/attrs")]);
        assert!(f.attr_tree);
        let env = Env::from_pairs([("GIT_CONFIG_PARAMETERS", "'include.path'='/x'")]);
        assert!(from_env(&env, Path::new("/r")).is_none());
    }
}
