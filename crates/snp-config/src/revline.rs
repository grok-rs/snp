//! autoupdate's `_original_lines` + `_write_new_config` (`commands/autoupdate.py:116-159`; 03 §11.3, 15 §A4.6,
//! vectors V-REV / V-REVW / V-AUL).
//!
//! The rev lines are found with `REV_LINE_RE` (the regex-free normative matcher of
//! `snp_pycompat::precommit::rev_line_match`) on `str.splitlines(True)` lines of the file read with
//! `newline=''`; the k-th matching line belongs to the k-th repo that is not `local`/`meta` (nor snp's
//! `builtin`, which has no `rev`). When the count differs, the file is re-dumped through PyYAML
//! (`yaml_dump(yaml_load(original))`, comments lost) and scanned once more; a second mismatch is pre-commit's
//! `AssertionError: could not find rev lines` (exit 3).
//!
//! Two data-safety fixes: a CRLF line keeps its `\r` when its comment is replaced (D-DATA-10), and a last line
//! without a newline counts as a line end, so such a file is not re-dumped (D-DATA-16).
//!
//! Owner: WP-21.

use std::borrow::Cow;
use std::sync::Arc;

use cfgv::{ValidationError, Value};
use snp_pycompat::precommit::rev_line_match;
use snp_pycompat::text::{splitlines_keepends, strip};
use snp_pycompat::textio::universal_newlines;
use snp_yaml::emit::{Quote, redump, scalar_for_rev};

use crate::items::{BUILTIN, LOCAL, META};
use crate::load::limits;
use crate::{ConfigError, messages};

/// One repo's new `rev` (and `# frozen: …` comment).
#[derive(Debug, Clone)]
pub struct RevUpdate {
    /// Index into `repos` (the config's whole list; `local`, `meta` and `builtin` entries have no rev line).
    pub repo_index: usize,
    /// The new rev.
    pub new_rev: Arc<str>,
    /// `# frozen: <tag>` when `--freeze`.
    pub frozen_comment: Option<Arc<str>>,
}

/// The rewritten file.
#[derive(Debug)]
#[non_exhaustive]
pub enum Rewrite {
    /// Line-preserving rewrite of the `rev:` lines.
    Lines(Vec<u8>),
    /// The PyYAML re-dump path (the rev lines could not be located).
    FullReformat(Vec<u8>),
}

fn invalid(msg: String) -> ConfigError {
    ConfigError::invalid(false, &ValidationError::new(msg))
}

/// The `repos` entries that have a rev line, as indices into `repos` (`config_repos`, `autoupdate.py:174-177`).
fn rev_repo_indices(text: &str) -> Result<Vec<usize>, ConfigError> {
    let value =
        snp_yaml::load(&universal_newlines(text), &limits()).map_err(|e| invalid(e.to_string()))?;
    let repos = value
        .as_map()
        .and_then(|m| m.get(&Value::from("repos")))
        .and_then(Value::as_array)
        .unwrap_or_default();
    Ok(repos
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let url = r
                .as_map()
                .and_then(|m| m.get(&Value::from("repo")))
                .and_then(Value::as_str);
            !matches!(url, Some(LOCAL | META | BUILTIN))
        })
        .map(|(i, _)| i)
        .collect())
}

/// A line split off `str.splitlines(True)`, with the D-DATA-16 virtual newline of an unterminated last line.
struct Line<'a> {
    text: Cow<'a, str>,
    virtual_newline: bool,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out: Vec<Line<'_>> = splitlines_keepends(text)
        .map(|l| Line {
            text: Cow::Borrowed(l),
            virtual_newline: false,
        })
        .collect();
    if let Some(last) = out.last_mut()
        && !last.text.ends_with(['\n', '\r'])
        && !last
            .text
            .chars()
            .last()
            .is_some_and(snp_pycompat::chars::is_linebreak)
    {
        last.text = Cow::Owned(format!("{}\n", last.text));
        last.virtual_newline = true;
    }
    out
}

/// The indices of the rev lines.
fn rev_lines(lines: &[Line<'_>]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| rev_line_match(&l.text).is_some())
        .map(|(i, _)| i)
        .collect()
}

/// `_write_new_config`'s line assembly with the D-DATA-10 fix.
fn rewrite_line(line: &str, u: &RevUpdate) -> Option<String> {
    let [g1, g2, g3, _g4, g5, g6] = rev_line_match(line)?;
    let quote = match g3 {
        "'" => Quote::Single,
        "\"" => Quote::Double,
        _ => Quote::Plain,
    };
    let new_rev = scalar_for_rev(&u.new_rev, quote);
    let comment = match &u.frozen_comment {
        Some(tag) => format!("  # frozen: {tag}"),
        None if strip(g5).starts_with("# frozen:") => String::new(),
        None => g5.to_owned(),
    };
    let cr = if comment != g5 && g5.ends_with('\r') {
        "\r"
    } else {
        ""
    };
    Some(format!("{g1}rev:{g2}{new_rev}{comment}{cr}{g6}"))
}

/// Applies `updates` to the rev lines of `text`; `None` when the rev lines do not line up with the repos.
fn apply(text: &str, repo_indices: &[usize], updates: &[RevUpdate]) -> Option<String> {
    let lines = lines(text);
    let idxs = rev_lines(&lines);
    if idxs.len() != repo_indices.len() {
        return None;
    }
    let mut out_lines: Vec<String> = lines.iter().map(|l| l.text.to_string()).collect();
    for u in updates {
        let Some(k) = repo_indices.iter().position(|i| *i == u.repo_index) else {
            continue;
        };
        let Some(&line_no) = idxs.get(k) else {
            continue;
        };
        let Some(slot) = out_lines.get_mut(line_no) else {
            continue;
        };
        if let Some(new) = rewrite_line(slot, u) {
            *slot = new;
        }
    }
    if let (Some(last), Some(l)) = (out_lines.last_mut(), lines.last())
        && l.virtual_newline
        && last.ends_with('\n')
    {
        last.pop();
    }
    Some(out_lines.concat())
}

/// Rewrites the `rev:` lines of `original` (EOL-preserving, idempotent).
///
/// `original` is the file's bytes (strict UTF-8, line endings as on disk). An update whose `repo_index` names a
/// repo without a rev line is ignored.
pub fn rewrite(original: &[u8], updates: &[RevUpdate]) -> Result<Rewrite, ConfigError> {
    let text = snp_pycompat::codec::decode_utf8(original).map_err(|e| invalid(e.to_string()))?;
    let repo_indices = rev_repo_indices(text)?;
    if let Some(out) = apply(text, &repo_indices, updates) {
        return Ok(Rewrite::Lines(out.into_bytes()));
    }
    let dumped =
        redump(&universal_newlines(text), &limits()).map_err(|e| invalid(e.to_string()))?;
    match apply(&dumped, &repo_indices, updates) {
        Some(out) => Ok(Rewrite::FullReformat(out.into_bytes())),
        None => Err(ConfigError {
            manifest: false,
            rendered: format!("AssertionError: {}", messages::COULD_NOT_FIND_REV_LINES),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(i: usize, rev: &str, frozen: Option<&str>) -> RevUpdate {
        RevUpdate {
            repo_index: i,
            new_rev: Arc::from(rev),
            frozen_comment: frozen.map(Arc::from),
        }
    }

    fn lines_of(r: Rewrite) -> String {
        match r {
            Rewrite::Lines(b) => String::from_utf8(b).unwrap(),
            Rewrite::FullReformat(b) => panic!("reformat: {}", String::from_utf8_lossy(&b)),
        }
    }

    #[test]
    fn missing_final_newline_is_a_line_end() {
        let src = "# keep me\nrepos:\n-   repo: r\n    hooks: []\n    rev: v1  # c";
        let out = lines_of(rewrite(src.as_bytes(), &[up(0, "v2", None)]).unwrap());
        assert_eq!(
            out,
            "# keep me\nrepos:\n-   repo: r\n    hooks: []\n    rev: v2  # c"
        );
    }

    #[test]
    fn crlf_kept_when_freezing() {
        let src = "repos:\r\n-   repo: r\r\n    rev: v1  # note\r\n    hooks: []\r\n";
        let out = lines_of(rewrite(src.as_bytes(), &[up(0, "abc", Some("v2"))]).unwrap());
        assert_eq!(
            out,
            "repos:\r\n-   repo: r\r\n    rev: abc  # frozen: v2\r\n    hooks: []\r\n"
        );
    }

    #[test]
    fn local_repos_are_skipped() {
        let src =
            "repos:\n-   repo: local\n    hooks: []\n-   repo: r\n    rev: v1\n    hooks: []\n";
        let out = lines_of(rewrite(src.as_bytes(), &[up(1, "v2", None)]).unwrap());
        assert_eq!(out, src.replace("v1", "v2"));
    }

    #[test]
    fn flow_style_is_reformatted() {
        let src = "repos: [{repo: r, rev: v1, hooks: [{id: foo}]}]\n";
        match rewrite(src.as_bytes(), &[up(0, "v2", None)]).unwrap() {
            Rewrite::FullReformat(b) => assert_eq!(
                String::from_utf8(b).unwrap(),
                "repos:\n-   repo: r\n    rev: v2\n    hooks:\n    -   id: foo\n"
            ),
            Rewrite::Lines(_) => panic!("expected a reformat"),
        }
    }
}
