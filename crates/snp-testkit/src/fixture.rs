//! pre-commit's `testing/resources` repositories and conftest scenarios.
//!
//! Owner: WP-18.

/// One variant per repository of pre-commit's `testing/resources` (10 §3, recreated verbatim under
/// `crates/snp-testkit/resources/<name>/`), plus the conftest scenario helpers of 10 §7.3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fixture {
    /// e.g. "`script_hooks_repo`", "`python_hooks_repo`" (10 §3 names).
    Resource(&'static str),
    /// `in_merge_conflict`.
    MergeConflict,
    /// `in_conflicting_submodule`.
    ConflictingSubmodule,
    /// `commit_msg_repo`.
    CommitMsgRepo,
    /// `prepare_commit_msg_repo` (`failing_prepare_commit_msg_repo`).
    PrepareCommitMsgRepo {
        /// The failing variant.
        failing: bool,
    },
    /// `push_example`.
    PushExample,
    /// `make_consuming_repo(<resource>)`.
    ConsumingRepo(&'static str),
    /// `out_of_date` (`tagged_repo` variant).
    OutOfDate {
        /// The upstream has a tag.
        tagged: bool,
    },
    /// `hook_disappearing`.
    HookDisappearing,
    /// `img_staged`.
    ImgStaged,
    /// `sub_staged`.
    SubStaged,
}
