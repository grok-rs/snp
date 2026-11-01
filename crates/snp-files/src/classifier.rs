//! `Classifier` (`commands/run.py`): top-level include/exclude, then per-hook filtering.
//!
//! Owner: WP-22.

use std::sync::Arc;

use file_identify::Meta;
use snp_core::failure::{ExcKind, Failure};
use snp_proc::pool::JobPool;
use snp_pyre::{MatchError, MatchKind, PyRegex};

use crate::table::{REGEX_THREADS, par_chunks_on};
use crate::{FileId, FileTable, HookFilter, TagOutcome, messages};

/// `Classifier.from_config`: top-level include/exclude (`re.search` on the `/`-path), then lexists.
#[derive(Debug)]
pub struct Classifier {
    /// The run's file table.
    pub table: Arc<FileTable>,
    /// Files kept by the top-level filters, in table order.
    pub kept: Vec<FileId>,
}

impl Classifier {
    /// Nested pool use runs inline (§1.4.8).
    ///
    /// `Classifier.from_config` + `__init__` (`run.py:73-74, 112-126`): keep a file when `include.search(path)` and
    /// not `exclude.search(path)` (on the matching view), then when `os.path.lexists(path)`. Order and duplicates are
    /// kept. Large tables are split into ordered partitions on the pool. The `lstat`s become the stat tier of the
    /// files the first hook classifies (pre-commit's first `tags_from_path` of those files happens right after).
    ///
    /// # Errors
    ///
    /// The backtrack-limit `FatalError` (exit 1, D-SEC-10) naming the pattern and the path of the first file (in
    /// table order) that hit it; `Failure::Interrupted` when the pool is cancelled.
    pub fn new(
        table: Arc<FileTable>,
        include: &PyRegex,
        exclude: &PyRegex,
        pool: &JobPool,
    ) -> Result<Self, Failure> {
        let t: &FileTable = &table;
        let found: Vec<(FileId, Meta)> = par_chunks_on(pool, t.len(), REGEX_THREADS, |range| {
            let mut out = Vec::new();
            for raw in range {
                let id = FileId(u32::try_from(raw).unwrap_or(u32::MAX));
                if passes(t, id, include, exclude)?
                    && let Some(meta) = t.lexists_meta(raw)
                {
                    out.push((id, meta));
                }
            }
            Ok(out)
        })?;
        let mut pending = vec![None; t.len()].into_boxed_slice();
        let mut kept = Vec::new();
        for (id, meta) in found {
            if let Some(slot) = t.index(id).and_then(|i| pending.get_mut(i)) {
                *slot = Some(meta);
            }
            kept.push(id);
        }
        t.set_pending(pending);
        Ok(Self { table, kept })
    }

    /// `filenames_for_hook`: order & duplicates kept; Vanished → dropped (X13-CLS-01); Unreadable → Err
    /// (X13-CLS-02, exit 3); a regex backtrack-limit hit → `FatalError` naming pattern and path (§3.3).
    ///
    /// `run.py:100-109`: the hook's `files`/`exclude` (`re.search`), then `by_types` — `types ⊆ tags ∧ (types_or = ∅
    /// ∨ tags ∩ types_or ≠ ∅) ∧ tags ∩ exclude_types = ∅` — computing only the tiers `f.needs` names. Python chains
    /// the two as generators, so each file is matched and then identified before the next one; snp does the same
    /// per partition (partitions run on the pool, results concatenated in order), so the first error in file order
    /// is the one reported. Files outside `f.prefix` never match. A file known to be gone is dropped for this and
    /// every later hook (D-CRASH-08); a file whose content is needed but cannot be read fails the run with
    /// pre-commit's exception text, exit 3 (fail closed; D-CRASH-18 limits it to hooks that need the content).
    ///
    /// # Errors
    ///
    /// The unreadable file's `OSError` (exit 3), the backtrack-limit `FatalError` (exit 1), or
    /// `Failure::Interrupted` when the pool is cancelled.
    pub fn for_hook(&self, f: &HookFilter, pool: &JobPool) -> Result<Vec<FileId>, Failure> {
        // The first hook after `Classifier::new` adopts its fresh `lstat`s (a hook that needs no tags discards them).
        let pending = self.table.take_pending();
        let adopt = pending.as_deref().filter(|_| f.needs.any());
        let table: &FileTable = &self.table;
        let kept = &self.kept;
        par_chunks_on(pool, kept.len(), REGEX_THREADS, |range| {
            let mut out = Vec::new();
            for &id in kept.get(range).unwrap_or_default() {
                if f.prefix
                    .as_ref()
                    .is_some_and(|p| table.relative_to(id, p).is_none())
                    || !passes(table, id, &f.files, &f.exclude)?
                {
                    continue;
                }
                if let Some(p) = adopt {
                    table.adopt(id, p);
                }
                if keep(table, id, f)? {
                    out.push(id);
                }
            }
            Ok(out)
        })
    }

    /// check-useless-excludes.
    ///
    /// `Classifier.by_types` alone (`run.py:81-98`, as `meta_hooks/check_useless_excludes.py:47-58` calls it): the
    /// type filters of `f` over `ids`, order kept; `files`, `exclude` and `prefix` are not consulted.
    ///
    /// # Errors
    ///
    /// As [`Classifier::for_hook`] (an unreadable file whose content the types need).
    pub fn by_types(&self, ids: &[FileId], f: &HookFilter) -> Result<Vec<FileId>, Failure> {
        let pending = self.table.take_pending();
        let adopt = pending.as_deref().filter(|_| f.needs.any());
        let mut out = Vec::new();
        for &id in ids {
            if let Some(p) = adopt {
                self.table.adopt(id, p);
            }
            if keep(&self.table, id, f)? {
                out.push(id);
            }
        }
        Ok(out)
    }
}

/// The type filter for one file: `Ok(false)` drops it (no match, or gone), `Err` fails the run (unreadable).
fn keep(table: &FileTable, id: FileId, f: &HookFilter) -> Result<bool, Failure> {
    // No type filter at all (`types: []`, the default `types_or`/`exclude_types`): every tag set matches, so only a
    // file already known to be gone is dropped; no tag needs computing.
    if f.types.is_empty() && f.types_or.is_empty() && f.exclude_types.is_empty() {
        return Ok(!table.known_vanished(id));
    }
    match table.tags(id, f.needs) {
        TagOutcome::Tags(tags) => Ok(tags.matches_types(&f.types, &f.types_or, &f.exclude_types)),
        TagOutcome::Vanished => Ok(false),
        TagOutcome::Unreadable(exc) => Err(Failure::Exc {
            kind: ExcKind::Unexpected,
            exc: exc.clone(),
        }),
    }
}

/// `include.search(path) and not exclude.search(path)` (`filter_by_include_exclude`, `run.py:60-70`).
fn passes(
    table: &FileTable,
    id: FileId,
    include: &PyRegex,
    exclude: &PyRegex,
) -> Result<bool, Failure> {
    let view = table.match_view(id);
    let search = |re: &PyRegex| match trivial_search(re, view) {
        Some(hit) => Ok(hit),
        None => re
            .search(view)
            .map_err(|_: MatchError| backtrack(table, id, re)),
    };
    Ok(search(include)? && !search(exclude)?)
}

/// `re.search` of the two patterns nearly every hook leaves at their defaults, answered without a regex call:
/// `''` matches everything, `^$` exactly `""` and `"\n"` (`$` also matches before a final newline).
fn trivial_search(re: &PyRegex, view: &str) -> Option<bool> {
    match re.kind() {
        MatchKind::All => Some(true),
        MatchKind::EmptyOrNewline => Some(view.is_empty() || view == "\n"),
        _ => None,
    }
}

/// D-SEC-10: `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on
/// {path!r}` (exit 1).
fn backtrack(table: &FileTable, id: FileId, re: &PyRegex) -> Failure {
    let path = if cfg!(unix) {
        snp_pycompat::repr::repr_fsbytes(table.bytes(id))
    } else {
        snp_pycompat::repr::repr_str(table.match_view(id))
    };
    Failure::fatal(
        messages::BACKTRACK_TY,
        messages::backtrack_limit(re.source(), &path),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The trivial-pattern shortcut agrees with `re.search` on the edge views.
    #[test]
    fn trivial_patterns_agree_with_search() {
        let views = ["", "\n", "\n\n", "a", "a\n", "\na", " ", "src/x.py"];
        for src in ["", "^$", "(?:)", "\\A\\Z", "x"] {
            let re = PyRegex::compile(src, snp_pyre::Options::CONFIG).unwrap();
            for v in views {
                if let Some(hit) = trivial_search(&re, v) {
                    assert_eq!(hit, re.search(v).unwrap(), "{src:?} on {v:?}");
                }
            }
        }
        let all = PyRegex::compile("", snp_pyre::Options::CONFIG).unwrap();
        let none = PyRegex::compile("^$", snp_pyre::Options::CONFIG).unwrap();
        assert_eq!(trivial_search(&all, "x"), Some(true));
        assert_eq!(trivial_search(&none, "\n"), Some(true));
        assert_eq!(trivial_search(&none, "x"), Some(false));
    }

    #[test]
    fn backtrack_failure_text() {
        let t = FileTable::from_args(vec!["aaab".into()], Path::new("/r"));
        let re = PyRegex::compile("(?:(?=a)a+)+(?<!a)b", snp_pyre::Options::CONFIG).unwrap();
        let f = backtrack(&t, FileId(0), &re);
        assert_eq!(f.exit_code(), 1);
        let Failure::Exc { kind, exc } = f else {
            panic!("not an exception")
        };
        assert_eq!(kind, ExcKind::Fatal);
        assert_eq!(&*exc.ty, "snp");
        assert_eq!(
            String::from_utf8_lossy(&exc.text),
            "regular expression '(?:(?=a)a+)+(?<!a)b' exceeded the backtracking limit on 'aaab'"
        );
    }
}
