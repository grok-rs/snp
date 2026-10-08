//! Git hook types and pre-commit stages (`clientlib.py:26-72`; 01 §1.2–§1.3).

/// A git hook type pre-commit can install (`HOOK_TYPES`), in source order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum HookType {
    /// `commit-msg`
    CommitMsg,
    /// `post-checkout`
    PostCheckout,
    /// `post-commit`
    PostCommit,
    /// `post-merge`
    PostMerge,
    /// `post-rewrite`
    PostRewrite,
    /// `pre-commit`
    PreCommit,
    /// `pre-merge-commit`
    PreMergeCommit,
    /// `pre-push`
    PrePush,
    /// `pre-rebase`
    PreRebase,
    /// `prepare-commit-msg`
    PrepareCommitMsg,
}

/// The positional argument count `hook-impl` accepts for a hook type
/// (`hook_impl._check_args_length`, 02 §16.2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArgCount {
    /// Exactly `n` arguments.
    Exact(u8),
    /// Between `min` and `max` arguments (inclusive).
    Range(u8, u8),
}

impl HookType {
    /// `HOOK_TYPES` in source order (01 §1.2): drives `--hook-type` choices and help output.
    pub const ALL: [Self; 10] = [
        Self::CommitMsg,
        Self::PostCheckout,
        Self::PostCommit,
        Self::PostMerge,
        Self::PostRewrite,
        Self::PreCommit,
        Self::PreMergeCommit,
        Self::PrePush,
        Self::PreRebase,
        Self::PrepareCommitMsg,
    ];

    /// The git hook name, e.g. `pre-commit`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommitMsg => "commit-msg",
            Self::PostCheckout => "post-checkout",
            Self::PostCommit => "post-commit",
            Self::PostMerge => "post-merge",
            Self::PostRewrite => "post-rewrite",
            Self::PreCommit => "pre-commit",
            Self::PreMergeCommit => "pre-merge-commit",
            Self::PrePush => "pre-push",
            Self::PreRebase => "pre-rebase",
            Self::PrepareCommitMsg => "prepare-commit-msg",
        }
    }

    /// Exact name match; legacy stage aliases are not hook types (01 §1.3).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.as_str() == s)
    }

    /// `hook_impl._check_args_length` (02 §16.2).
    pub const fn arg_count(self) -> ArgCount {
        match self {
            Self::PrepareCommitMsg => ArgCount::Range(1, 3),
            Self::PreRebase => ArgCount::Range(1, 2),
            Self::PostCheckout => ArgCount::Exact(3),
            Self::PrePush => ArgCount::Exact(2),
            Self::CommitMsg | Self::PostMerge | Self::PostRewrite => ArgCount::Exact(1),
            Self::PostCommit | Self::PreCommit | Self::PreMergeCommit => ArgCount::Exact(0),
        }
    }

    /// The stage a hook of this type runs in.
    pub const fn stage(self) -> Stage {
        Stage::Hook(self)
    }

    const fn index(self) -> u8 {
        match self {
            Self::CommitMsg => 0,
            Self::PostCheckout => 1,
            Self::PostCommit => 2,
            Self::PostMerge => 3,
            Self::PostRewrite => 4,
            Self::PreCommit => 5,
            Self::PreMergeCommit => 6,
            Self::PrePush => 7,
            Self::PreRebase => 8,
            Self::PrepareCommitMsg => 9,
        }
    }
}

/// A pre-commit stage (`STAGES = (*HOOK_TYPES, 'manual')`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The stage of a git hook.
    Hook(HookType),
    /// `manual`: never triggered by a git hook.
    Manual,
}

impl Stage {
    /// `STAGES` in source order, `manual` last (01 §1.2).
    pub const ALL: [Self; 11] = [
        Self::Hook(HookType::CommitMsg),
        Self::Hook(HookType::PostCheckout),
        Self::Hook(HookType::PostCommit),
        Self::Hook(HookType::PostMerge),
        Self::Hook(HookType::PostRewrite),
        Self::Hook(HookType::PreCommit),
        Self::Hook(HookType::PreMergeCommit),
        Self::Hook(HookType::PrePush),
        Self::Hook(HookType::PreRebase),
        Self::Hook(HookType::PrepareCommitMsg),
        Self::Manual,
    ];

    /// The stage name, e.g. `pre-push` or `manual`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hook(h) => h.as_str(),
            Self::Manual => "manual",
        }
    }

    /// Canonical names only (no legacy aliases).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// `clientlib.transform_stage`: `commit` → `pre-commit`, `push` → `pre-push`,
    /// `merge-commit` → `pre-merge-commit`; anything else unchanged (01 §1.3).
    pub fn transform_legacy(s: &str) -> &str {
        match s {
            "commit" => "pre-commit",
            "push" => "pre-push",
            "merge-commit" => "pre-merge-commit",
            other => other,
        }
    }

    /// [`Stage::parse`] after [`Stage::transform_legacy`]; the flag says whether a legacy alias was
    /// used (it triggers pre-commit's deprecation warning where pre-commit accepts aliases).
    pub fn parse_with_legacy(s: &str) -> Option<(Self, bool)> {
        let t = Self::transform_legacy(s);
        Self::parse(t).map(|st| (st, t != s))
    }

    const fn bit(self) -> u16 {
        let i = match self {
            Self::Hook(h) => h.index(),
            Self::Manual => 10,
        };
        1 << i
    }
}

/// A set of stages (one bit per [`Stage::ALL`] entry).
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct StageSet(u16);

impl StageSet {
    /// Every stage.
    pub const ALL: Self = Self(0b111_1111_1111);
    /// No stage.
    pub const EMPTY: Self = Self(0);

    /// `self ∪ {s}`.
    #[must_use]
    pub const fn with(self, s: Stage) -> Self {
        Self(self.0 | s.bit())
    }

    /// `s ∈ self`.
    pub const fn contains(self, s: Stage) -> bool {
        self.0 & s.bit() != 0
    }

    /// Whether no stage is in the set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The members in [`Stage::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Stage> {
        Stage::ALL.into_iter().filter(move |s| self.contains(*s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_types_in_source_order() {
        let names: Vec<&str> = HookType::ALL.iter().map(|h| h.as_str()).collect();
        assert_eq!(
            names,
            [
                "commit-msg",
                "post-checkout",
                "post-commit",
                "post-merge",
                "post-rewrite",
                "pre-commit",
                "pre-merge-commit",
                "pre-push",
                "pre-rebase",
                "prepare-commit-msg"
            ]
        );
        let mut sorted = HookType::ALL;
        sorted.sort_unstable();
        assert_eq!(sorted, HookType::ALL, "Ord follows source order");
        for h in HookType::ALL {
            assert_eq!(HookType::parse(h.as_str()), Some(h));
            assert_eq!(h.stage(), Stage::Hook(h));
        }
        assert_eq!(HookType::parse("commit"), None);
        assert_eq!(HookType::parse("manual"), None);
    }

    #[test]
    fn stages_in_source_order_manual_last() {
        let names: Vec<&str> = Stage::ALL.iter().map(|s| s.as_str()).collect();
        let mut want: Vec<&str> = HookType::ALL.iter().map(|h| h.as_str()).collect();
        want.push("manual");
        assert_eq!(names, want);
        assert_eq!(Stage::parse("manual"), Some(Stage::Manual));
        assert_eq!(Stage::parse("commit"), None);
    }

    #[test]
    fn legacy_aliases() {
        assert_eq!(
            Stage::parse_with_legacy("commit"),
            Some((Stage::Hook(HookType::PreCommit), true))
        );
        assert_eq!(
            Stage::parse_with_legacy("push"),
            Some((Stage::Hook(HookType::PrePush), true))
        );
        assert_eq!(
            Stage::parse_with_legacy("merge-commit"),
            Some((Stage::Hook(HookType::PreMergeCommit), true))
        );
        assert_eq!(
            Stage::parse_with_legacy("pre-commit"),
            Some((Stage::Hook(HookType::PreCommit), false))
        );
        assert_eq!(
            Stage::parse_with_legacy("manual"),
            Some((Stage::Manual, false))
        );
        assert_eq!(Stage::parse_with_legacy("bogus"), None);
        assert_eq!(Stage::transform_legacy("x"), "x");
    }

    #[test]
    fn arg_counts() {
        assert_eq!(
            HookType::PrepareCommitMsg.arg_count(),
            ArgCount::Range(1, 3)
        );
        assert_eq!(HookType::PreRebase.arg_count(), ArgCount::Range(1, 2));
        assert_eq!(HookType::PostCheckout.arg_count(), ArgCount::Exact(3));
        assert_eq!(HookType::PrePush.arg_count(), ArgCount::Exact(2));
        assert_eq!(HookType::CommitMsg.arg_count(), ArgCount::Exact(1));
        assert_eq!(HookType::PreCommit.arg_count(), ArgCount::Exact(0));
    }

    #[test]
    fn stage_sets() {
        assert!(StageSet::EMPTY.is_empty() && StageSet::default().is_empty());
        let s = StageSet::EMPTY
            .with(Stage::Manual)
            .with(Stage::Hook(HookType::CommitMsg));
        assert!(s.contains(Stage::Manual) && !s.contains(Stage::Hook(HookType::PrePush)));
        assert_eq!(
            s.iter().collect::<Vec<_>>(),
            vec![Stage::Hook(HookType::CommitMsg), Stage::Manual]
        );
        assert_eq!(
            StageSet::ALL.iter().collect::<Vec<_>>(),
            Stage::ALL.to_vec()
        );
        assert!(Stage::ALL.iter().all(|st| StageSet::ALL.contains(*st)));
    }
}
