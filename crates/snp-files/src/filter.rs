//! A hook's compiled filter (`files`, `exclude`, `types`, `types_or`, `exclude_types`, prefix).
//!
//! Owner: WP-22.

use std::sync::{Arc, OnceLock};

use file_identify::{Tag, TagSet};
use snp_core::path::RepoPath;
use snp_pyre::{Options, PyReError, PyRegex, RegexCache};

use crate::Needs;

/// `Classifier.filenames_for_hook`'s inputs, compiled.
#[derive(Debug, Clone)]
pub struct HookFilter {
    /// `files` (`re.search`).
    pub files: Arc<PyRegex>,
    /// `exclude` (`re.search`).
    pub exclude: Arc<PyRegex>,
    /// `types`.
    pub types: TagSet,
    /// `types_or`.
    pub types_or: TagSet,
    /// `exclude_types`.
    pub exclude_types: TagSet,
    /// The identify tiers the type filters need.
    pub needs: Needs,
    /// Boundary-aware path prefix (the `root` extension; the v1.1 workspace project).
    pub prefix: Option<RepoPath>,
}

impl HookFilter {
    /// Five filter fields as plain arguments (snp-files does not depend on snp-config) plus the boundary-aware path
    /// `prefix` (the `root` extension in v1; the workspace project prefix in v1.1): files outside it never match.
    ///
    /// `files`/`exclude` are compiled through `cache` with `Options::CONFIG` (`re.compile`, `run.py:60-70`; engines
    /// are built on first match). `needs` follows from the referenced tags (ADR-041): any tag at all needs the stat
    /// tier (identify reports a symlink, directory or socket by its type tag alone, without its name tags, so even
    /// a name-derived tag is exact only once the kind is known); `text`/`binary` need the encoding tier; a tag some
    /// shebang interpreter yields (`python`, `shell`, …, identify's `INTERPRETERS`) needs the shebang tier.
    ///
    /// # Errors
    ///
    /// The pattern's compile error (config validation has normally rejected it already).
    pub fn compile(
        files: &str,
        exclude: &str,
        types: TagSet,
        types_or: TagSet,
        exclude_types: TagSet,
        prefix: Option<RepoPath>,
        cache: &RegexCache,
    ) -> Result<Self, PyReError> {
        let files = cache.get(files, Options::CONFIG)?;
        let exclude = cache.get(exclude, Options::CONFIG)?;
        Ok(Self {
            files,
            exclude,
            types,
            types_or,
            exclude_types,
            needs: needs_for(types.union(types_or).union(exclude_types)),
            prefix,
        })
    }
}

/// Every tag a shebang interpreter can contribute (the union of identify's `INTERPRETERS` values).
fn interpreter_tags() -> TagSet {
    static TAGS: OnceLock<TagSet> = OnceLock::new();
    *TAGS.get_or_init(|| {
        file_identify::interpreters::INTERPRETERS
            .values()
            .fold(TagSet::EMPTY, TagSet::union)
    })
}

/// The tiers a filter referencing `referenced` needs.
pub(crate) fn needs_for(referenced: TagSet) -> Needs {
    let encoding = TagSet::from_tag(Tag::TEXT).with(Tag::BINARY);
    Needs {
        stat: !referenced.is_empty(),
        encoding: referenced.intersects(&encoding),
        shebang: referenced.intersects(&interpreter_tags()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> TagSet {
        TagSet::from_names(names).unwrap()
    }

    #[test]
    fn needs_from_referenced_tags() {
        assert_eq!(needs_for(TagSet::EMPTY), Needs::default());
        assert_eq!(
            needs_for(tags(&["file"])),
            Needs {
                stat: true,
                encoding: false,
                shebang: false
            }
        );
        assert_eq!(
            needs_for(tags(&["text"])),
            Needs {
                stat: true,
                encoding: true,
                shebang: false
            }
        );
        let py = needs_for(tags(&["python"]));
        assert!(py.stat && py.shebang && !py.encoding);
        let sh = needs_for(tags(&["shell", "binary"]));
        assert!(sh.stat && sh.shebang && sh.encoding);
        // `yaml` comes only from names, never from a shebang.
        assert!(!needs_for(tags(&["yaml"])).shebang);
        assert!(!needs_for(tags(&["executable", "symlink", "directory"])).shebang);
    }

    #[test]
    fn compile_uses_the_cache() {
        let cache = RegexCache::new();
        let f = HookFilter::compile(
            r"\.py$",
            "^$",
            tags(&["file"]),
            TagSet::EMPTY,
            TagSet::EMPTY,
            None,
            &cache,
        )
        .unwrap();
        let g = HookFilter::compile(
            r"\.py$",
            "^$",
            tags(&["python"]),
            TagSet::EMPTY,
            TagSet::EMPTY,
            RepoPath::new(b"sub"),
            &cache,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&f.files, &g.files));
        assert!(f.needs.stat && !f.needs.encoding && !f.needs.shebang);
        assert!(g.needs.shebang);
        assert!(
            HookFilter::compile(
                "(",
                "",
                TagSet::EMPTY,
                TagSet::EMPTY,
                TagSet::EMPTY,
                None,
                &cache
            )
            .is_err()
        );
    }
}
