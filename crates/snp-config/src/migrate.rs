//! `migrate-config` (`commands/migrate_config.py`, `yaml_rewrite.py`; 01 §10, D-DATA-09/14/15, D-CRASH-07/25).
//!
//! pre-commit's algorithm with three data-safety fixes: the file is read and written as UTF-8 on every platform
//! and each line keeps its own line ending (D-DATA-09, D-CRASH-25; the caller replaces the file atomically keeping
//! its mode), a node reached twice through an alias is rewritten once (D-DATA-14), and a block scalar (`|`, `>`)
//! is replaced by a plain scalar instead of pre-commit's `>python>` (D-DATA-15). Every failure is an
//! `InvalidConfigError` under `File <config>` (D-CRASH-07), never a crash.
//!
//! Owner: WP-21.

use std::ffi::OsStr;
use std::path::Path;

use cfgv::{ValidationError, Value};
use snp_pycompat::precommit::migrate_header_split;
use snp_pycompat::textio::{textwrap_indent, universal_newlines};
use snp_yaml::{Document, NodeId, NodeKind, ScalarStyle, Schema};

use crate::ConfigError;
use crate::load::{self, limits};

/// What `migrate_config` did.
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
    /// The new file bytes (`Configuration has been migrated.`).
    Migrated(Vec<u8>),
    /// Nothing to do (`Configuration is already migrated.`).
    AlreadyMigrated,
}

/// `Configuration has been migrated.` (printed even when quiet, `migrate_config.py:130-132`).
pub const MIGRATED: &str = crate::messages::MIGRATED;
/// `Configuration is already migrated.` (not printed when quiet).
pub const ALREADY_MIGRATED: &str = crate::messages::ALREADY_MIGRATED;

/// Reads the config for `migrate-config` / `autoupdate`: a missing (or non-regular) file is the validation error
/// `run` prints (`\n=====> {display} is not a file`, exit 1) instead of pre-commit's `FileNotFoundError` crash
/// (X13-CFG-10, D-CRASH-07); an unreadable file stays the uncaught `OSError` (X13-CFG-09).
pub fn read_config(path: &Path, display: &OsStr) -> Result<Vec<u8>, ConfigError> {
    load::read_file(path, &display.to_string_lossy(), false)
}

/// `migrate_config(config_file)`: top-level list → `repos:`, `sha:` → `rev:`, legacy stage names, `python_venv`.
pub fn migrate_config(bytes: &[u8], display: &OsStr) -> Result<Outcome, ConfigError> {
    let display = display.to_string_lossy();
    let fail = |msg: String| {
        ConfigError::invalid(
            false,
            &ValidationError::new(msg).with_context(format!("File {display}")),
        )
    };
    let orig = snp_pycompat::codec::decode_utf8(bytes).map_err(|e| fail(e.to_string()))?;
    let loaded =
        snp_yaml::load(&universal_newlines(orig), &limits()).map_err(|e| fail(e.to_string()))?;
    let mapped = if matches!(loaded, Value::List(_)) {
        migrate_map(orig)
    } else {
        orig.to_owned()
    };
    let contents = migrate_composed(&mapped).map_err(fail)?;
    if contents == orig {
        Ok(Outcome::AlreadyMigrated)
    } else {
        Ok(Outcome::Migrated(contents.into_bytes()))
    }
}

/// The line ending of the first line (`\r\n` or `\n`) — the ending of the inserted `repos:` line.
fn first_eol(s: &str) -> &'static str {
    match s.find('\n') {
        Some(i) if i > 0 && s.as_bytes().get(i.wrapping_sub(1)) == Some(&b'\r') => "\r\n",
        _ => "\n",
    }
}

/// `_migrate_map` (`migrate_config.py:25-46`): header lines kept, `repos:` inserted, the rest unchanged when the
/// result parses, else indented by four spaces.
fn migrate_map(contents: &str) -> String {
    let (header, rest) = migrate_header_split(contents);
    let eol = first_eol(contents);
    let trial = format!("{header}repos:{eol}{rest}");
    if snp_yaml::load(&universal_newlines(&trial), &limits()).is_ok() {
        trial
    } else {
        format!("{header}repos:{eol}{}", textwrap_indent(rest, "    "))
    }
}

/// One step of a `yaml_rewrite` matcher chain.
#[derive(Copy, Clone)]
enum Step {
    /// `MappingValue(k)`.
    Value(&'static str),
    /// `MappingKey(k)`.
    Key(&'static str),
    /// `SequenceItem()`.
    Item,
}

/// `yaml_rewrite.match(tree, matcher)`: the scalar nodes the chain reaches, in document order (aliases are the
/// same node ids, so a node can be reached twice).
fn match_chain(doc: &Document, root: NodeId, chain: &[Step]) -> Vec<NodeId> {
    let node = |id: NodeId| doc.nodes.get(usize::try_from(id.0).unwrap_or(usize::MAX));
    let scalar_is = |id: NodeId, k: &str| {
        node(id).is_some_and(|n| matches!(&n.kind, NodeKind::Scalar(v) if &**v == k))
    };
    let mut current = vec![root];
    for step in chain {
        let mut next = Vec::new();
        for id in current {
            let Some(n) = node(id) else { continue };
            match (step, &n.kind) {
                (Step::Value(k), NodeKind::Map(pairs)) => {
                    next.extend(
                        pairs
                            .iter()
                            .filter(|(key, _)| scalar_is(*key, k))
                            .map(|(_, v)| *v),
                    );
                }
                (Step::Key(k), NodeKind::Map(pairs)) => {
                    next.extend(
                        pairs
                            .iter()
                            .filter(|(key, _)| scalar_is(*key, k))
                            .map(|(key, _)| *key),
                    );
                }
                (Step::Item, NodeKind::Seq(items)) => next.extend(items.iter().copied()),
                _ => {}
            }
        }
        current = next;
    }
    current.retain(|id| node(*id).is_some_and(|n| matches!(n.kind, NodeKind::Scalar(_))));
    current
}

/// The start of a node's scalar token within its span: past any `&anchor` / `!tag` properties.
fn token_start(text: &str, span: &std::ops::Range<usize>) -> usize {
    let mut pos = span.start;
    loop {
        let rest = text.get(pos..span.end).unwrap_or("");
        if !(rest.starts_with('&') || rest.starts_with('!')) {
            return pos;
        }
        let prop = rest.find([' ', '\t', '\r', '\n']).unwrap_or(rest.len());
        let after = rest.get(prop..).unwrap_or("");
        let ws = after
            .len()
            .saturating_sub(after.trim_start_matches([' ', '\t', '\r', '\n']).len());
        pos = pos.saturating_add(prop).saturating_add(ws);
    }
}

/// The replacement of one scalar token: `_preserve_style`, except that a block scalar becomes a plain scalar
/// followed by the line breaks and indentation its token ended with (D-DATA-15).
fn replacement(style: ScalarStyle, token: &str, new: &str) -> String {
    match style {
        ScalarStyle::Single => format!("'{new}'"),
        ScalarStyle::Double => format!("\"{new}\""),
        ScalarStyle::Literal | ScalarStyle::Folded => {
            let trimmed = token.trim_end_matches([' ', '\t', '\r', '\n']);
            let tail = token.get(trimmed.len()..).unwrap_or("");
            format!("{new}{tail}")
        }
        _ => new.to_owned(),
    }
}

/// `_migrate_composed` (`migrate_config.py:58-111`) over the composed node tree with byte spans.
fn migrate_composed(contents: &str) -> Result<String, String> {
    let doc = match snp_yaml::compose(contents, Schema::PyYaml11Safe, &limits()) {
        Ok(doc) => doc,
        Err(e) => {
            // pre-commit composes the universal-newline text: report its error text.
            let normalized = universal_newlines(contents);
            return Err(
                snp_yaml::compose(&normalized, Schema::PyYaml11Safe, &limits())
                    .err()
                    .unwrap_or(e)
                    .to_string(),
            );
        }
    };
    let Some(root) = doc.root else {
        return Ok(contents.to_owned());
    };
    let mut rewrites: Vec<(NodeId, String)> = Vec::new();
    let value_of = |id: NodeId| -> Option<&str> {
        match &doc.nodes.get(usize::try_from(id.0).ok()?)?.kind {
            NodeKind::Scalar(v) => Some(v),
            _ => None,
        }
    };
    let repos_hooks = [
        Step::Value("repos"),
        Step::Item,
        Step::Value("hooks"),
        Step::Item,
    ];
    for id in match_chain(
        &doc,
        root,
        &[Step::Value("repos"), Step::Item, Step::Key("sha")],
    ) {
        rewrites.push((id, "rev".to_owned()));
    }
    let mut language = repos_hooks.to_vec();
    language.push(Step::Value("language"));
    for id in match_chain(&doc, root, &language) {
        if value_of(id) == Some("python_venv") {
            rewrites.push((id, "python".to_owned()));
        }
    }
    let mut hook_stages = repos_hooks.to_vec();
    hook_stages.extend([Step::Value("stages"), Step::Item]);
    let stage_ids = match_chain(&doc, root, &[Step::Value("default_stages"), Step::Item])
        .into_iter()
        .chain(match_chain(&doc, root, &hook_stages));
    for id in stage_ids {
        if let Some(v) = value_of(id)
            && crate::items::LEGACY_STAGES.contains(&v)
        {
            rewrites.push((id, format!("pre-{v}")));
        }
    }
    // D-DATA-14: one rewrite per node (an aliased node is reached once per alias).
    let mut seen = Vec::new();
    rewrites.retain(|(id, _)| {
        if seen.contains(id) {
            false
        } else {
            seen.push(*id);
            true
        }
    });
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (id, new) in rewrites {
        let Some(n) = doc.nodes.get(usize::try_from(id.0).unwrap_or(usize::MAX)) else {
            continue;
        };
        let start = token_start(contents, &n.span);
        let token = contents.get(start..n.span.end).unwrap_or("");
        spans.push((start, n.span.end, replacement(n.style, token, &new)));
    }
    spans.sort_by_key(|(start, _, _)| *start);
    let mut out = String::new();
    let mut pos = 0_usize;
    for (start, end, new) in spans {
        if start < pos {
            continue;
        }
        out.push_str(contents.get(pos..start).unwrap_or(""));
        out.push_str(&new);
        pos = end;
    }
    out.push_str(contents.get(pos..).unwrap_or(""));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated(src: &str) -> String {
        match migrate_config(src.as_bytes(), OsStr::new(".pre-commit-config.yaml")).unwrap() {
            Outcome::Migrated(b) => String::from_utf8(b).unwrap(),
            Outcome::AlreadyMigrated => src.to_owned(),
        }
    }

    #[test]
    fn token_start_skips_properties() {
        let t = "a: &x !!str commit";
        assert_eq!(token_start(t, &(3..t.len())), 12);
        assert_eq!(token_start("commit", &(0..6)), 0);
    }

    #[test]
    fn aliases_are_rewritten_once() {
        let src = "default_stages: &s [commit]\nrepos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n        stages: *s\n";
        assert_eq!(migrated(src), src.replace("[commit]", "[pre-commit]"),);
    }

    #[test]
    fn block_scalars_become_plain() {
        let src = "repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: >-\n            python_venv\n        files: a\n";
        let out = migrated(src);
        assert_eq!(
            out,
            "repos:\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: python\n        files: a\n"
        );
    }

    #[test]
    fn crlf_is_kept() {
        let src = "-   repo: local\r\n    hooks:\r\n    -   id: x\r\n        name: x\r\n        entry: x\r\n        language: system\r\n        stages: [commit]\r\n";
        let out = migrated(src);
        assert!(out.starts_with("repos:\r\n-   repo: local\r\n"));
        assert!(out.contains("stages: [pre-commit]\r\n"));
        assert!(!out.replace("\r\n", "").contains('\n'));
    }
}
