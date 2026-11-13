//! `main.py` argument handling (`main.py:366-375`): argparse over the brand's spec, the `help` re-parse, then the
//! typed [`crate::Cli`] facade (12 §A.5, §A.17).
//!
//! Owner: WP-12.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;

use snp_argparse::{Namespace, Outcome, ParseCfg, ParserSpec};
use snp_core::output::ColorSetting;
use snp_core::stage::{HookType, Stage};
use snp_core::{brand::Brand, env::Env};

use crate::args::{
    AutoupdateArgs, CacheAction, CacheArgs, Cli, CliOutcome, ColorArgs, ConfigArg, ConfigArgs,
    ConfigCmd, DoctorArgs, HazmatArgs, HazmatTool, HookImplArgs, InitTemplatedirArgs, InstallArgs,
    InstallHooksArgs, ListArgs, MigrateArgs, RunArgs, RunExt, SelfArgs, Shell, StdinFormat,
    TryRepoArgs, UninstallArgs, ValidateArgs,
};
use crate::specs::{PRE_COMMIT_SPEC, SNP_SPEC};

/// argv WITHOUT `argv[0]`; `[]` ⇒ `["run"]`; `help [cmd]` re-parses `[cmd, --help]` (12 §A.5). Never touches disk.
///
/// The compat brand parses with [`PRE_COMMIT_SPEC`] and prog `pre-commit`, the snp brand with [`SNP_SPEC`] and
/// prog `snp` (ADR-023, ADR-058). Help and `--version` text → [`CliOutcome::Stdout`] (exit 0); usage errors →
/// [`CliOutcome::Usage`] (stderr, exit 2). `stdout_tty_cols` is the width of the terminal on stdout (`None` when
/// stdout is not a terminal); `COLUMNS` wins over it (12 §A.3). The texts use `\n`: the caller writes them through
/// Python's text layer (`Printer::py_print`), which turns them into CRLF on Windows like `sys.stdout`/`sys.stderr` do.
/// `hazmat` without a tool, pre-commit's `AttributeError` crash (exit 3), is a usage error here (D-CRASH-22).
pub fn parse(
    brand: Brand,
    argv: &[OsString],
    env: &Env,
    stdout_tty_cols: Option<usize>,
) -> CliOutcome {
    let spec = spec_for(brand);
    let cfg = ParseCfg {
        prog: brand.prog(),
        env,
        columns: snp_argparse::columns(env, stdout_tty_cols),
    };
    let ns = match parse_namespace(spec, argv, &cfg) {
        Ok(ns) => ns,
        Err(outcome) => return outcome,
    };
    facade(spec, &ns, &cfg)
}

/// The spec tree of a brand.
pub(crate) const fn spec_for(brand: Brand) -> &'static ParserSpec {
    match brand {
        Brand::PreCommit => PRE_COMMIT_SPEC,
        Brand::Snp => SNP_SPEC,
    }
}

/// `main.py:366-375` up to the parsed namespace: the `[] → ['run']` rewrite, `parse_args`, and the `help`
/// command's re-parse. `Err` carries help/version/usage outcomes.
pub(crate) fn parse_namespace(
    spec: &'static ParserSpec,
    argv: &[OsString],
    cfg: &ParseCfg<'_>,
) -> Result<Namespace, CliOutcome> {
    let run = [OsString::from("run")];
    let argv = if argv.is_empty() { &run[..] } else { argv };
    let ns = outcome(snp_argparse::parse(spec, argv, cfg))?;
    let Some(("help", help)) = ns.command() else {
        return Ok(ns);
    };
    // `parser.parse_args([args.help_cmd, '--help'])` / `parser.parse_args(['--help'])`: always exits.
    let again: Vec<OsString> = match help.value("help_cmd").filter(|c| !c.is_empty()) {
        Some(cmd) => vec![cmd.to_os_string(), OsString::from("--help")],
        None => vec![OsString::from("--help")],
    };
    match outcome(snp_argparse::parse(spec, &again, cfg)) {
        Err(o) => Err(o),
        // Unreachable: `--help` ends every re-parse. Show the top-level help rather than dispatching `help`.
        Ok(_) => Err(CliOutcome::Stdout(snp_argparse::format_help(
            spec,
            &[],
            cfg,
        ))),
    }
}

fn outcome(o: Outcome) -> Result<Namespace, CliOutcome> {
    match o {
        Outcome::Parsed(ns) => Ok(ns),
        Outcome::Help(text) | Outcome::Version(text) => Err(CliOutcome::Stdout(text)),
        Outcome::Usage { stderr } => Err(CliOutcome::Usage(stderr)),
        _ => Err(CliOutcome::Usage(String::new())),
    }
}

/// A usage error reported by the parser at `path` (its usage block, then `{prog}: error: {message}`).
fn usage_error(
    spec: &'static ParserSpec,
    path: &[&str],
    cfg: &ParseCfg<'_>,
    message: &str,
) -> CliOutcome {
    let usage = snp_argparse::format_usage(spec, path, cfg);
    let mut prog = cfg.prog.to_owned();
    for p in path {
        prog.push(' ');
        prog.push_str(p);
    }
    CliOutcome::Usage(format!("{usage}{prog}: error: {message}\n"))
}

// ----- namespace → typed facade -----------------------------------------------------------------------------

fn color(ns: &Namespace) -> ColorSetting {
    ns.value("color")
        .and_then(ColorSetting::parse)
        .unwrap_or(ColorSetting::Auto)
}

fn os(ns: &Namespace, dest: &str) -> OsString {
    ns.value(dest).map_or_default(OsStr::to_os_string)
}

/// `-c/--config`; not given ⇒ [`ConfigArg::absent`] (discovered by the commands, ADR-087).
fn config(ns: &Namespace) -> ConfigArg {
    match ns.value("config") {
        Some(c) if ns.was_given("config") => ConfigArg::given(c),
        _ => ConfigArg::absent(),
    }
}

/// `Some` only for a non-empty value: pre-commit tests these options for truthiness (`run.py:262-414`).
fn truthy(ns: &Namespace, dest: &str) -> Option<OsString> {
    ns.value(dest)
        .filter(|v| !v.is_empty())
        .map(OsStr::to_os_string)
}

/// `Some` whenever the option was given (`is not None` tests: `--commit-msg-filename`, `--ref`, `--hook-dir`).
fn given(ns: &Namespace, dest: &str) -> Option<OsString> {
    ns.value(dest).map(OsStr::to_os_string)
}

fn hook_types(ns: &Namespace) -> Vec<HookType> {
    ns.list("hook_types")
        .iter()
        .filter_map(|t| t.to_str().and_then(HookType::parse))
        .collect()
}

fn stage(v: Option<&OsStr>) -> Option<Stage> {
    v.and_then(OsStr::to_str).and_then(Stage::parse)
}

/// `-j N` of `snp run`: `0` (and negative values) mean "the number of CPUs", like autoupdate's `-j 0`.
fn jobs(ns: &Namespace) -> Option<NonZeroUsize> {
    ns.int("jobs")
        .and_then(|n| usize::try_from(n).ok())
        .and_then(NonZeroUsize::new)
}

fn run_args(ns: &Namespace) -> RunArgs {
    // compat: `hook` is `nargs='?'` (a string or None); snp brand: `nargs='*'`. An empty id is falsy in
    // `run.py:427`, i.e. "no hook given".
    let hooks: Vec<OsString> = match ns.value("hook") {
        Some(h) => vec![h.to_os_string()],
        None => ns.list("hook").to_vec(),
    }
    .into_iter()
    .filter(|h| !h.is_empty())
    .collect();
    RunArgs {
        color: color(ns),
        config: config(ns),
        hooks,
        verbose: ns.flag("verbose"),
        all_files: ns.flag("all_files"),
        files: ns.list("files").to_vec(),
        show_diff_on_failure: ns.flag("show_diff_on_failure"),
        fail_fast: ns.flag("fail_fast"),
        hook_stage: stage(ns.value("hook_stage")).unwrap_or(Stage::Hook(HookType::PreCommit)),
        remote_branch: truthy(ns, "remote_branch"),
        local_branch: truthy(ns, "local_branch"),
        from_ref: truthy(ns, "from_ref"),
        to_ref: truthy(ns, "to_ref"),
        pre_rebase_upstream: truthy(ns, "pre_rebase_upstream"),
        pre_rebase_branch: truthy(ns, "pre_rebase_branch"),
        commit_msg_filename: given(ns, "commit_msg_filename"),
        prepare_commit_message_source: truthy(ns, "prepare_commit_message_source"),
        commit_object_name: truthy(ns, "commit_object_name"),
        remote_name: truthy(ns, "remote_name"),
        remote_url: truthy(ns, "remote_url"),
        checkout_type: truthy(ns, "checkout_type"),
        is_squash_merge: truthy(ns, "is_squash_merge"),
        rewrite_command: truthy(ns, "rewrite_command"),
        ext: RunExt {
            jobs: jobs(ns),
            groups: ns.list("groups").to_vec(),
            require_groups: ns.list("require_groups").to_vec(),
            no_groups: ns.list("no_groups").to_vec(),
            no_fast_path: ns.flag("no_fast_path"),
            stdin_files: match ns.value("stdin_files").map(OsStr::as_encoded_bytes) {
                Some(b"nul") => Some(StdinFormat::Nul),
                Some(_) => Some(StdinFormat::Lines),
                None => None,
            },
            summary: ns.flag("summary"),
            no_local_config: ns.flag("no_local_config"),
        },
    }
}

/// The sub-command of `ns` (`args.<dest>`) and its namespace.
fn sub(ns: &Namespace) -> Option<(&'static str, &Namespace)> {
    ns.command()
}

/// The typed command for a parsed namespace; `hazmat` without a tool is a usage error (D-CRASH-22: pre-commit
/// crashes with `AttributeError`, exit 3).
pub(crate) fn facade(spec: &'static ParserSpec, top: &Namespace, cfg: &ParseCfg<'_>) -> CliOutcome {
    let Some((name, ns)) = top.command() else {
        // Unreachable: an empty argv is `run`, and a successful parse of anything else selects a command.
        return usage_error(
            spec,
            &[],
            cfg,
            "the following arguments are required: command",
        );
    };
    let cli = match name {
        "run" => Cli::Run(run_args(ns)),
        "try-repo" => Cli::TryRepo(TryRepoArgs {
            repo: os(ns, "repo"),
            rev: given(ns, "ref"),
            run: run_args(ns),
        }),
        "hook-impl" => Cli::HookImpl(HookImplArgs {
            color: color(ns),
            config: config(ns),
            hook_type: os(ns, "hook_type"),
            hook_dir: given(ns, "hook_dir"),
            skip_on_missing_config: ns.flag("skip_on_missing_config"),
            rest: ns.list("rest").to_vec(),
        }),
        "install" => Cli::Install(InstallArgs {
            color: color(ns),
            config: config(ns),
            overwrite: ns.flag("overwrite"),
            install_hooks: ns.flag("install_hooks"),
            hook_types: hook_types(ns),
            allow_missing_config: ns.flag("allow_missing_config"),
            git_config_hooks: ns.flag("git_config_hooks"),
        }),
        "uninstall" => Cli::Uninstall(UninstallArgs {
            color: color(ns),
            config: config(ns),
            hook_types: hook_types(ns),
            git_config_hooks: ns.flag("git_config_hooks"),
        }),
        "install-hooks" => Cli::InstallHooks(InstallHooksArgs {
            color: color(ns),
            config: config(ns),
            with_fallback: ns.flag("with_fallback"),
        }),
        "init-templatedir" => Cli::InitTemplatedir(InitTemplatedirArgs {
            color: color(ns),
            config: config(ns),
            directory: os(ns, "directory"),
            no_allow_missing_config: !ns.flag("allow_missing_config"),
            hook_types: hook_types(ns),
        }),
        "autoupdate" => Cli::Autoupdate(AutoupdateArgs {
            color: color(ns),
            config: config(ns),
            bleeding_edge: ns.flag("bleeding_edge"),
            freeze: ns.flag("freeze"),
            repos: ns.list("repos").to_vec(),
            jobs: ns.int("jobs").unwrap_or(1),
        }),
        "clean" => Cli::Clean(ColorArgs { color: color(ns) }),
        "gc" => Cli::Gc(ColorArgs { color: color(ns) }),
        "sample-config" => Cli::SampleConfig(ColorArgs { color: color(ns) }),
        "migrate-config" => Cli::MigrateConfig(ConfigArgs {
            color: color(ns),
            config: config(ns),
        }),
        "validate-config" | "validate-manifest" => {
            let args = ValidateArgs {
                color: color(ns),
                filenames: ns.list("filenames").to_vec(),
                strict: ns.flag("strict"),
                portable: ns.flag("portable"),
            };
            if name == "validate-config" {
                Cli::ValidateConfig(args)
            } else {
                Cli::ValidateManifest(args)
            }
        }
        "hazmat" => {
            let tool = match sub(ns) {
                Some(("cd", t)) => HazmatTool::Cd {
                    subdir: os(t, "subdir"),
                    cmd: t.list("cmd").to_vec(),
                },
                Some(("ignore-exit-code", t)) => HazmatTool::IgnoreExitCode {
                    cmd: t.list("cmd").to_vec(),
                },
                Some(("n1", t)) => HazmatTool::N1 {
                    cmd: t.list("cmd").to_vec(),
                },
                _ => {
                    return usage_error(
                        spec,
                        &["hazmat"],
                        cfg,
                        "the following arguments are required: tool",
                    );
                }
            };
            Cli::Hazmat(HazmatArgs {
                color: color(ns),
                tool,
            })
        }
        "list" => Cli::List(ListArgs {
            config: config(ns),
            json: ns.flag("json"),
            stage: stage(ns.value("hook_stage")),
            groups: ns.list("groups").to_vec(),
            require_groups: ns.list("require_groups").to_vec(),
            no_groups: ns.list("no_groups").to_vec(),
        }),
        "config" => match sub(ns) {
            Some(("dump", d)) => Cli::Config(ConfigCmd::Dump {
                config: config(d),
                json: d.flag("json"),
                no_local_config: d.flag("no_local_config"),
            }),
            _ => Cli::Config(ConfigCmd::Schema),
        },
        "cache" => Cli::Cache(CacheArgs {
            action: match sub(ns).map(|s| s.0) {
                Some("size") => CacheAction::Size,
                Some("gc") => CacheAction::Gc,
                Some("clean") => CacheAction::Clean,
                _ => CacheAction::Dir,
            },
        }),
        "migrate" => Cli::Migrate(match sub(ns) {
            Some(("lefthook", m)) => MigrateArgs::Lefthook {
                root: given(m, "root"),
                config: given(m, "config"),
                output: given(m, "output"),
                portable: m.flag("portable"),
                local: m.flag("local"),
                strict: m.flag("strict"),
                install: m.flag("install"),
                force: m.flag("force"),
            },
            Some(("upstream", m)) => MigrateArgs::Upstream { config: config(m) },
            Some((_, m)) => MigrateArgs::Builtin { config: config(m) },
            None => MigrateArgs::Builtin {
                config: ConfigArg::absent(),
            },
        }),
        "doctor" => Cli::Doctor(DoctorArgs {
            fix: ns.flag("fix"),
            fast_path: ns.flag("fast_path"),
            stash: ns.flag("stash"),
            config: config(ns),
        }),
        "self" => Cli::SelfCmd(match sub(ns) {
            Some(("alias", a)) => match sub(a) {
                Some(("install", i)) => SelfArgs::AliasInstall {
                    dir: given(i, "dir"),
                    force: i.flag("force"),
                },
                Some(("uninstall", _)) => SelfArgs::AliasUninstall,
                _ => SelfArgs::AliasStatus,
            },
            _ => SelfArgs::Check,
        }),
        "completions" => Cli::Completions(match ns.value("shell").map(OsStr::as_encoded_bytes) {
            Some(b"zsh") => Shell::Zsh,
            Some(b"fish") => Shell::Fish,
            Some(b"powershell") => Shell::PowerShell,
            _ => Shell::Bash,
        }),
        other => {
            // Unreachable for the two built-in trees (`help` was re-parsed above).
            return usage_error(
                spec,
                &[],
                cfg,
                &format!("argument command: invalid choice: {other:?}"),
            );
        }
    };
    CliOutcome::Cli(cli)
}
