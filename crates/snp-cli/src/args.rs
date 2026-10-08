//! The typed CLI facade: one struct per command, one field per argparse dest of 12 §A.8.1.
//!
//! Owner: WP-12.

use std::ffi::OsString;
use std::num::NonZeroUsize;

use snp_core::output::ColorSetting;
use snp_core::stage::{HookType, Stage};

/// Result of [`crate::parse`].
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "frozen §1.4.4 shape; built once per process"
)]
pub enum CliOutcome {
    /// A command to dispatch.
    Cli(Cli),
    /// Help or version text for stdout (exit 0).
    Stdout(String),
    /// Usage error text for stderr (exit 2).
    Usage(String),
}

/// One parsed command (`args.command`).
#[derive(Debug)]
#[non_exhaustive]
pub enum Cli {
    /// `run`.
    Run(RunArgs),
    /// `try-repo`.
    TryRepo(TryRepoArgs),
    /// `hook-impl` (hidden).
    HookImpl(HookImplArgs),
    /// `install`.
    Install(InstallArgs),
    /// `uninstall`.
    Uninstall(UninstallArgs),
    /// `install-hooks`.
    InstallHooks(InstallHooksArgs),
    /// `init-templatedir`.
    InitTemplatedir(InitTemplatedirArgs),
    /// `autoupdate`.
    Autoupdate(AutoupdateArgs),
    /// `clean`.
    Clean(ColorArgs),
    /// `gc`.
    Gc(ColorArgs),
    /// `migrate-config`.
    MigrateConfig(ConfigArgs),
    /// `sample-config`.
    SampleConfig(ColorArgs),
    /// `validate-config`.
    ValidateConfig(ValidateArgs),
    /// `validate-manifest`.
    ValidateManifest(ValidateArgs),
    /// `hazmat`.
    Hazmat(HazmatArgs),
    /// `list` (snp brand only; absent from the compat brand's choices, help and parser, ADR-023).
    List(ListArgs),
    /// `config` (snp brand only).
    Config(ConfigCmd),
    /// `cache` (snp brand only).
    Cache(CacheArgs),
    /// `migrate` (snp brand only).
    Migrate(MigrateArgs),
    /// `doctor` (snp brand only).
    Doctor(DoctorArgs),
    /// `self` (snp brand only).
    SelfCmd(SelfArgs),
    /// `completions` (snp brand only).
    Completions(Shell),
}

/// Commands with only `--color`.
#[derive(Clone, Debug)]
pub struct ColorArgs {
    /// `--color`.
    pub color: ColorSetting,
}

/// Commands with `--color` and `-c/--config`.
#[derive(Clone, Debug)]
pub struct ConfigArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
}

/// One field per argparse dest of 12 §A.8.1; `Some("")` ref options normalised to None (Python truthiness).
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per argparse dest (12 §A.8.1)"
)]
pub struct RunArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `hook` positional(s) (compat: ≤ 1).
    pub hooks: Vec<OsString>,
    /// `-v/--verbose`.
    pub verbose: bool,
    /// `-a/--all-files`.
    pub all_files: bool,
    /// `--files`.
    pub files: Vec<OsString>,
    /// `--show-diff-on-failure`.
    pub show_diff_on_failure: bool,
    /// `--fail-fast`.
    pub fail_fast: bool,
    /// `--hook-stage`.
    pub hook_stage: Stage,
    /// `--remote-branch`.
    pub remote_branch: Option<OsString>,
    /// `--local-branch`.
    pub local_branch: Option<OsString>,
    /// `--from-ref` / `--source` / `-s`.
    pub from_ref: Option<OsString>,
    /// `--to-ref` / `--origin` / `-o`.
    pub to_ref: Option<OsString>,
    /// `--pre-rebase-upstream`.
    pub pre_rebase_upstream: Option<OsString>,
    /// `--pre-rebase-branch`.
    pub pre_rebase_branch: Option<OsString>,
    /// `--commit-msg-filename`.
    pub commit_msg_filename: Option<OsString>,
    /// `--prepare-commit-message-source`.
    pub prepare_commit_message_source: Option<OsString>,
    /// `--commit-object-name`.
    pub commit_object_name: Option<OsString>,
    /// `--remote-name`.
    pub remote_name: Option<OsString>,
    /// `--remote-url`.
    pub remote_url: Option<OsString>,
    /// `--checkout-type`.
    pub checkout_type: Option<OsString>,
    /// `--is-squash-merge`.
    pub is_squash_merge: Option<OsString>,
    /// `--rewrite-command`.
    pub rewrite_command: Option<OsString>,
    /// snp brand extensions.
    pub ext: RunExt,
}

/// snp brand flags of `run` (§9.3); env equivalents applied by snp-run.
#[derive(Clone, Debug, Default)]
pub struct RunExt {
    /// `-j/--jobs`.
    pub jobs: Option<NonZeroUsize>,
    /// `--group` (OR).
    pub groups: Vec<OsString>,
    /// `--require-group` (AND).
    pub require_groups: Vec<OsString>,
    /// `--no-group` (exclusion wins; `@ungrouped` allowed in all three).
    pub no_groups: Vec<OsString>,
    /// `--no-fast-path`.
    pub no_fast_path: bool,
    /// `--stdin-files[=nul|lines]`.
    pub stdin_files: Option<StdinFormat>,
    /// `--summary`.
    pub summary: bool,
    /// `--no-local-config`.
    pub no_local_config: bool,
}

/// `--stdin-files` record separator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StdinFormat {
    /// NUL-separated (`git … -z`).
    Nul,
    /// Newline-separated.
    Lines,
}

/// `try-repo`.
#[derive(Clone, Debug)]
pub struct TryRepoArgs {
    /// `repo` positional.
    pub repo: OsString,
    /// `--ref` / `--rev`.
    pub rev: Option<OsString>,
    /// The `run` options.
    pub run: RunArgs,
}

/// `hook-impl` (hidden).
#[derive(Clone, Debug)]
pub struct HookImplArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `--hook-type` (free string).
    pub hook_type: OsString,
    /// `--hook-dir`.
    pub hook_dir: Option<OsString>,
    /// `--skip-on-missing-config`.
    pub skip_on_missing_config: bool,
    /// Raw REMAINDER, `--` kept.
    pub rest: Vec<OsString>,
}

/// `install`.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per argparse dest (12 §A.8.1)"
)]
pub struct InstallArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `-f/--overwrite`.
    pub overwrite: bool,
    /// `--install-hooks`.
    pub install_hooks: bool,
    /// `-t/--hook-type`.
    pub hook_types: Vec<HookType>,
    /// `--allow-missing-config`.
    pub allow_missing_config: bool,
    /// snp brand `--git-config-hooks` (ADR-057).
    pub git_config_hooks: bool,
}

/// `uninstall`.
#[derive(Clone, Debug)]
pub struct UninstallArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `-t/--hook-type`.
    pub hook_types: Vec<HookType>,
    /// snp brand `--git-config-hooks` (ADR-057).
    pub git_config_hooks: bool,
}

/// `install-hooks`.
#[derive(Clone, Debug)]
pub struct InstallHooksArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// snp brand `--with-fallback`.
    pub with_fallback: bool,
}

/// `init-templatedir`.
#[derive(Clone, Debug)]
pub struct InitTemplatedirArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `directory` positional.
    pub directory: OsString,
    /// `--no-allow-missing-config`.
    pub no_allow_missing_config: bool,
    /// `-t/--hook-type`.
    pub hook_types: Vec<HookType>,
}

/// `autoupdate`.
#[derive(Clone, Debug)]
pub struct AutoupdateArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `--bleeding-edge`.
    pub bleeding_edge: bool,
    /// `--freeze`.
    pub freeze: bool,
    /// `--repo` (append).
    pub repos: Vec<OsString>,
    /// `-j/--jobs`.
    pub jobs: i64,
}

/// `validate-config` / `validate-manifest`.
#[derive(Clone, Debug)]
pub struct ValidateArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `filenames` positional.
    pub filenames: Vec<OsString>,
    /// snp brand `--strict`.
    pub strict: bool,
    /// snp brand `--portable`.
    pub portable: bool,
}

/// `hazmat`.
#[derive(Clone, Debug)]
pub struct HazmatArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// The hazmat tool.
    pub tool: HazmatTool,
}

/// `hazmat` sub-commands (03 H.8).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum HazmatTool {
    /// `hazmat cd <subdir> <cmd…>`.
    Cd {
        /// `subdir` positional.
        subdir: OsString,
        /// REMAINDER command.
        cmd: Vec<OsString>,
    },
    /// `hazmat ignore-exit-code <cmd…>`.
    IgnoreExitCode {
        /// REMAINDER command.
        cmd: Vec<OsString>,
    },
    /// `hazmat n1 <cmd…>`.
    N1 {
        /// REMAINDER command.
        cmd: Vec<OsString>,
    },
}

/// `list` (snp brand).
#[derive(Clone, Debug)]
pub struct ListArgs {
    /// `-c/--config`.
    pub config: OsString,
    /// `--json`.
    pub json: bool,
    /// `--hook-stage`.
    pub stage: Option<Stage>,
    /// `--group`.
    pub groups: Vec<OsString>,
    /// `--require-group`.
    pub require_groups: Vec<OsString>,
    /// `--no-group`.
    pub no_groups: Vec<OsString>,
}

/// `config` (snp brand).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ConfigCmd {
    /// `config dump`.
    Dump {
        /// `-c/--config`.
        config: OsString,
        /// `--json`.
        json: bool,
        /// `--no-local-config`.
        no_local_config: bool,
    },
    /// `config schema`.
    Schema,
}

/// `cache` (snp brand).
#[derive(Clone, Debug)]
pub struct CacheArgs {
    /// The action.
    pub action: CacheAction,
}

/// `cache dir|size|gc|clean`.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum CacheAction {
    /// `cache dir`.
    Dir,
    /// `cache size`.
    Size,
    /// `cache gc`.
    Gc,
    /// `cache clean`.
    Clean,
}

/// `migrate` (snp brand).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum MigrateArgs {
    /// `migrate lefthook`.
    Lefthook {
        /// `--root`.
        root: Option<OsString>,
        /// `--config`.
        config: Option<OsString>,
        /// `--output`.
        output: Option<OsString>,
        /// `--portable`.
        portable: bool,
        /// `--local`.
        local: bool,
        /// `--strict`.
        strict: bool,
        /// `--install`.
        install: bool,
        /// `--force`.
        force: bool,
    },
    /// `migrate builtin`.
    Builtin {
        /// `-c/--config`.
        config: OsString,
    },
    /// `migrate upstream`.
    Upstream {
        /// `-c/--config`.
        config: OsString,
    },
}

/// `doctor` (snp brand).
#[derive(Clone, Debug)]
pub struct DoctorArgs {
    /// `--fix`.
    pub fix: bool,
    /// `--fast-path`.
    pub fast_path: bool,
    /// `--stash`.
    pub stash: bool,
    /// `-c/--config`.
    pub config: OsString,
}

/// `self` (snp brand).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SelfArgs {
    /// `self alias install`.
    AliasInstall {
        /// `--dir`.
        dir: Option<OsString>,
        /// `--force`.
        force: bool,
    },
    /// `self alias uninstall`.
    AliasUninstall,
    /// `self alias status`.
    AliasStatus,
    /// `self check`.
    Check,
}

/// `completions <shell>` (snp brand).
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Shell {
    /// bash.
    Bash,
    /// zsh.
    Zsh,
    /// fish.
    Fish,
    /// `PowerShell`.
    PowerShell,
}
