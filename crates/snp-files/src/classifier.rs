//! `Classifier` (`commands/run.py`): top-level include/exclude, then per-hook filtering.
//!
//! Owner: WP-22.

use std::sync::Arc;

use snp_core::failure::{ExcKind, Failure};
use snp_proc::pool::JobPool;
use snp_pyre::{MatchError, PyRegex};

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
    /// kept. The `lstat`s run on the pool for large tables; their results become the stat tier of the files the
    /// first hook classifies (pre-commit's first `tags_from_path` of those files happens right after).
    ///
    /// # Errors
    ///
    /// The backtrack-limit `FatalError` (exit 1, D-SEC-10) naming the pattern and the path.
    pub fn new(
        table: Arc<FileTable>,
        include: &PyRegex,
        exclude: &PyRegex,
        pool: &JobPool,
    ) -> Result<Self, Failure> {
        let mut candidates = Vec::new();
        for raw in 0..table.len() {
            let id = FileId(u32::try_from(raw).unwrap_or(u32::MAX));
            if passes(&table, id, include, exclude)? {
                candidates.push(id);
            }
        }
        let metas = lexists_all(&table, &candidates, pool);
        let mut pending = vec![None; table.len()].into_boxed_slice();
        let mut kept = Vec::new();
        for (&id, meta) in candidates.iter().zip(metas) {
            if let Some(meta) = meta {
                kept.push(id);
                if let Some(slot) = table.index(id).and_then(|i| pending.get_mut(i)) {
                    *slot = Some(meta);
                }
            }
        }
        table.set_pending(pending);
        Ok(Self { table, kept })
    }

    /// `filenames_for_hook`: order & duplicates kept; Vanished → dropped (X13-CLS-01); Unreadable → Err
    /// (X13-CLS-02, exit 3); a regex backtrack-limit hit → `FatalError` naming pattern and path (§3.3).
    ///
    /// `run.py:100-109`: the hook's `files`/`exclude` (`re.search`), then `by_types` — `types ⊆ tags ∧ (types_or = ∅
    /// ∨ tags ∩ types_or ≠ ∅) ∧ tags ∩ exclude_types = ∅` — computing only the tiers `f.needs` names
    /// (prefetched on the pool). Files outside `f.prefix` never match. A file known to be gone is dropped for this
    /// and every later hook (D-CRASH-08); a file whose content is needed but cannot be read fails the run with
    /// pre-commit's exception text, exit 3 (fail closed; D-CRASH-18 limits it to hooks that need the content).
    ///
    /// # Errors
    ///
    /// The unreadable file's `OSError` (exit 3) or the backtrack-limit `FatalError` (exit 1).
    pub fn for_hook(&self, f: &HookFilter, pool: &JobPool) -> Result<Vec<FileId>, Failure> {
        let mut candidates = Vec::new();
        for &id in &self.kept {
            if f
                .prefix
                .as_ref()
                .is_some_and(|p| self.table.relative_to(id, p).is_none())
            {
                continue;
            }
            if passes(&self.table, id, &f.files, &f.exclude)? {
                candidates.push(id);
            }
        }
        // The first hook after `Classifier::new` adopts the fresh `lstat`s (or discards them when it needs none).
        self.table
            .adopt_pending(if f.needs.any() { &candidates } else { &[] });
        self.table.prefetch(&candidates, f.needs, pool);
        self.select(&candidates, f)
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
        self.table
            .adopt_pending(if f.needs.any() { ids } else { &[] });
        self.select(ids, f)
    }

    /// The type filter over `ids`.
    fn select(&self, ids: &[FileId], f: &HookFilter) -> Result<Vec<FileId>, Failure> {
        let mut out = Vec::new();
        for &id in ids {
            match self.table.tags(id, f.needs) {
                TagOutcome::Tags(tags) => {
                    if tags.matches_types(&f.types, &f.types_or, &f.exclude_types) {
                        out.push(id);
                    }
                }
                TagOutcome::Vanished => {}
                TagOutcome::Unreadable(exc) => {
                    return Err(Failure::Exc {
                        kind: ExcKind::Unexpected,
                        exc: exc.clone(),
                    });
                }
            }
        }
        Ok(out)
    }
}

/// `include.search(path) and not exclude.search(path)` (`filter_by_include_exclude`, `run.py:60-70`).
fn passes(table: &FileTable, id: FileId, include: &PyRegex, exclude: &PyRegex) -> Result<bool, Failure> {
    let view = table.match_view(id);
    let search = |re: &PyRegex| {
        re.search(view)
            .map_err(|_: MatchError| backtrack(table, id, re))
    };
    Ok(search(include)? && !search(exclude)?)
}

/// D-SEC-10: `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on
/// {path!r}` (exit 1).
fn backtrack(table: &FileTable, id: FileId, re: &PyRegex) -> Failure {
    let path = if cfg!(unix) {
        snp_pycompat::repr::repr_fsbytes(table.bytes(id))
    } else {
        snp_pycompat::repr::repr_str(table.match_view(id))
    };
    Failure::fatal(messages::BACKTRACK_TY, messages::backtrack_limit(re.source(), &path))
}

/// `lexists` of every candidate (the trusted mode or one `lstat`), on the pool for large lists.
fn lexists_all(
    table: &FileTable,
    ids: &[FileId],
    pool: &JobPool,
) -> Vec<Option<file_identify::Meta>> {
    const PARALLEL_MIN: usize = 512;
    let one = |id: FileId| table.index(id).and_then(|i| table.lexists_meta(i));
    let permits = pool.permits();
    if ids.len() < PARALLEL_MIN || permits.get() == 1 || JobPool::holds_permit() {
        return ids.iter().map(|&id| one(id)).collect();
    }
    let per = ids.len().div_ceil(permits.get().saturating_mul(4)).max(128);
    let chunks = ids.len().div_ceil(per);
    let parts = pool.map_ordered(permits, chunks, |k| {
        let start = k.saturating_mul(per);
        let end = start.saturating_add(per).min(ids.len());
        Ok(ids
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|&id| one(id))
            .collect::<Vec<_>>())
    });
    let mut out = Vec::new();
    for (k, part) in parts.into_iter().enumerate() {
        if let Ok(v) = part {
            out.extend(v);
        } else {
            // Cancelled or refused: do the partition inline (the run is stopping anyway; stay exact).
            let start = k.saturating_mul(per);
            let end = start.saturating_add(per).min(ids.len());
            out.extend(ids.get(start..end).unwrap_or_default().iter().map(|&id| one(id)));
        }
    }
    out
}
