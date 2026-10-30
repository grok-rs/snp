//! The typed CLI facade: one struct per command, one field per argparse dest of 12 §A.8.1 (`pre_commit/main.py`
//! `argparse.Namespace` attributes, converted: `--color` to [`ColorSetting`], `--hook-stage` to [`Stage`], `-t` to
//! [`HookType`]; argv strings stay `OsString`).
//!
//! Owner: WP-12.

use std::ffi::OsString;
use std::num::NonZeroUsize;

use snp_core::output::ColorSetting;
use snp_core::stage::{HookType, Stage};

/// Result of [`crate::parse`]: what `main.py:366-375` does with argv before it touches the store.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "frozen §1.4.4 shape; built once per process"
)]
pub enum CliOutcome {
    /// A command to dispatch (`args.command` and its options).
    Cli(Cli),
    /// Help or version text for stdout (argparse `print_help` / `version`, exit 0).
    Stdout(String),
    /// Usage error text for stderr (argparse `error()`: usage block + `{prog}: error: …`, exit 2).
    Usage(String),
}

/// One parsed command (`args.command`, dispatched by `main.py:383-445`).
#[derive(Debug)]
#[non_exhaustive]
pub enum Cli {
    /// `run` (`commands/run.py`).
    Run(RunArgs),
    /// `try-repo` (`commands/try_repo.py`).
    TryRepo(TryRepoArgs),
    /// `hook-impl` (hidden; `commands/hook_impl.py`).
    HookImpl(HookImplArgs),
    /// `install` (`commands/install_uninstall.py:install`).
    Install(InstallArgs),
    /// `uninstall` (`commands/install_uninstall.py:uninstall`).
    Uninstall(UninstallArgs),
    /// `install-hooks` (`commands/install_uninstall.py:install_hooks`).
    InstallHooks(InstallHooksArgs),
    /// `init-templatedir` (`commands/init_templatedir.py`).
    InitTemplatedir(InitTemplatedirArgs),
    /// `autoupdate` (`commands/autoupdate.py`).
    Autoupdate(AutoupdateArgs),
    /// `clean` (`commands/clean.py`).
    Clean(ColorArgs),
    /// `gc` (`commands/gc.py`).
    Gc(ColorArgs),
    /// `migrate-config` (`commands/migrate_config.py`).
    MigrateConfig(ConfigArgs),
    /// `sample-config` (`commands/sample_config.py`).
    SampleConfig(ColorArgs),
    /// `validate-config` (`commands/validate_config.py`).
    ValidateConfig(ValidateArgs),
    /// `validate-manifest` (`commands/validate_manifest.py`).
    ValidateManifest(ValidateArgs),
    /// `hazmat` (`commands/hazmat.py`); the tool is required here (D-CRASH-22).
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

/// Commands with only `--color` (`clean`, `gc`, `sample-config`; `color.py:add_color_option`).
#[derive(Clone, Debug)]
pub struct ColorArgs {
    /// `--color`.
    pub color: ColorSetting,
}

/// Commands with `--color` and `-c/--config` (`migrate-config`; `main.py:_add_config_option`).
#[derive(Clone, Debug)]
pub struct ConfigArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
}

/// `run` / `try-repo` options (`main.py:_add_run_options`, 64-172): one field per argparse dest of 12 §A.8.1;
/// `Some("")` ref options normalised to None (Python truthiness, `run.py:262-414`), `--commit-msg-filename` kept as
/// given (`main.py:181` tests it with `is not None`).
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
    /// `hook` positional(s) (compat: ≤ 1); empty ids dropped (`if not args.hook`, `run.py:427`).
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

/// snp brand flags of `run` / `try-repo` (ARCHITECTURE §9.3, COMPATIBILITY §8.1); env equivalents applied by snp-run.
#[derive(Clone, Debug, Default)]
pub struct RunExt {
    /// `-j/--jobs`; `None` when absent or ≤ 0 (the number of CPUs, like `autoupdate -j 0`).
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

/// `try-repo` (`main.py:316-330`, `commands/try_repo.py`).
#[derive(Clone, Debug)]
pub struct TryRepoArgs {
    /// `repo` positional.
    pub repo: OsString,
    /// `--ref` / `--rev` (kept when empty: `_repo_ref` tests `ref is not None`).
    pub rev: Option<OsString>,
    /// The `run` options.
    pub run: RunArgs,
}

/// `hook-impl` (hidden; `main.py:356-365`, `commands/hook_impl.py`).
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
    /// Raw REMAINDER, `--` kept; `main.py:408` passes `rest[1:]` to `hook_impl`.
    pub rest: Vec<OsString>,
}

/// `install` (`main.py:273-293`).
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
    /// `-t/--hook-type` (append; empty when not given, i.e. `None` in pre-commit).
    pub hook_types: Vec<HookType>,
    /// `--allow-missing-config`.
    pub allow_missing_config: bool,
    /// snp brand `--git-config-hooks` (ADR-057).
    pub git_config_hooks: bool,
}

/// `uninstall` (`main.py:333-338`).
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

/// `install-hooks` (`main.py:295-303`).
#[derive(Clone, Debug)]
pub struct InstallHooksArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// snp brand `--with-fallback` (ARCHITECTURE §7.2: also provision the delegate environments of native hooks).
    pub with_fallback: bool,
}

/// `init-templatedir` (`main.py:252-271`).
#[derive(Clone, Debug)]
pub struct InitTemplatedirArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// `-c/--config`.
    pub config: OsString,
    /// `directory` positional.
    pub directory: OsString,
    /// `--no-allow-missing-config` given (`store_false` into `allow_missing_config`).
    pub no_allow_missing_config: bool,
    /// `-t/--hook-type`.
    pub hook_types: Vec<HookType>,
}

/// `autoupdate` (`main.py:220-243`).
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
    /// `-j/--jobs` (Python `int()`, saturated; default 1; autoupdate clamps it).
    pub jobs: i64,
}

/// `validate-config` / `validate-manifest` (`main.py:340-348`).
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

/// `hazmat` (`main.py:247-250`, `commands/hazmat.py:add_parsers`).
#[derive(Clone, Debug)]
pub struct HazmatArgs {
    /// `--color`.
    pub color: ColorSetting,
    /// The hazmat tool.
    pub tool: HazmatTool,
}

/// `hazmat` sub-commands (03 H.8, `commands/hazmat.py:10-28`); `cmd` is the raw REMAINDER (`--` kept).
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

/// `list` (snp brand; ARCHITECTURE §9.3, the group selectors of ADR-080).
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

/// `config` (snp brand; ARCHITECTURE §9.3).
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

/// `cache` (snp brand; ARCHITECTURE §5.6, §9.3).
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

/// `migrate` (snp brand; 05 §19, 09 §7.1).
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

/// `doctor` (snp brand; 11 §A.15).
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

/// `self` (snp brand; 11 §A.9.2).
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

/// `completions <shell>` (snp brand; the scripts of [`crate::COMPLETIONS`], ARCHITECTURE §9.1).
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
