//! The static `ParserSpec` trees: pre-commit 4.6.2 (`main.py:202-365`, verbatim help strings, 12 §A.8.1) and the
//! snp brand (compat ⊕ the extensions of ARCHITECTURE §9.3 / COMPATIBILITY §8.1, prefix-stable), plus the
//! generated shell completions.
//!
//! Every action is a `const` so the two trees (and the planted fixtures of the prefix-stability tests) share
//! them; the order of each `actions` array is pre-commit's `add_argument` order.
//!
//! Owner: WP-12.

use snp_argparse::{ActionSpec, Conv, DefaultValue, Kind, Nargs, ParserSpec, SubCommand};

use crate::Shell;

// ----- building blocks -------------------------------------------------------------------------------------------

/// `add_argument(*opts, dest=dest)`: a `store` action taking one value, no help.
pub(crate) const fn store(opts: &'static [&'static str], dest: &'static str) -> ActionSpec {
    ActionSpec {
        opts,
        dest,
        nargs: Nargs::One,
        kind: Kind::Store,
        conv: Conv::Identity,
        choices: None,
        default: DefaultValue::None,
        required: false,
        help: None,
        metavar: None,
        mutex_group: None,
        hidden: false,
    }
}

/// `add_argument(*opts, action='store_true', help=help)`.
pub(crate) const fn store_true(
    opts: &'static [&'static str],
    dest: &'static str,
    help: Option<&'static str>,
) -> ActionSpec {
    ActionSpec {
        nargs: Nargs::Zero,
        kind: Kind::StoreTrue,
        default: DefaultValue::False,
        help,
        ..store(opts, dest)
    }
}

/// `add_argument(dest, nargs=nargs, help=help)`: a positional.
pub(crate) const fn positional(
    dest: &'static str,
    nargs: Nargs,
    help: Option<&'static str>,
) -> ActionSpec {
    ActionSpec {
        nargs,
        help,
        ..store(&[], dest)
    }
}

/// `add_subparsers(dest=dest, required=required)` with its commands.
pub(crate) const fn subparsers(
    dest: &'static str,
    required: bool,
    commands: &'static [SubCommand],
) -> ActionSpec {
    ActionSpec {
        nargs: Nargs::Parser,
        kind: Kind::SubParsers(commands),
        required,
        ..store(&[], dest)
    }
}

/// `ArgumentParser(prog=…)` of a sub-command (`add_parser(name)`; prog = parent prog + ` name`).
pub(crate) const fn parser(
    prog_suffix: &'static str,
    actions: &'static [ActionSpec],
) -> ParserSpec {
    ParserSpec {
        prog_suffix,
        description: None,
        epilog: None,
        actions,
        allow_abbrev: true,
        add_help: true,
    }
}

/// `subparsers.add_parser(name, help=help)`.
pub(crate) const fn cmd(
    name: &'static str,
    help: Option<&'static str>,
    spec: &'static ParserSpec,
) -> SubCommand {
    SubCommand {
        name,
        help,
        listed: true,
        spec,
    }
}

// ----- shared options (main.py:50-172, color.py:103-109) -------------------------------------------------------

/// `clientlib.HOOK_TYPES` (`clientlib.py:26-37`).
pub(crate) const HOOK_TYPES: &[&str] = &[
    "commit-msg",
    "post-checkout",
    "post-commit",
    "post-merge",
    "post-rewrite",
    "pre-commit",
    "pre-merge-commit",
    "pre-push",
    "pre-rebase",
    "prepare-commit-msg",
];

/// `clientlib.STAGES = (*HOOK_TYPES, 'manual')`.
pub(crate) const STAGES: &[&str] = &[
    "commit-msg",
    "post-checkout",
    "post-commit",
    "post-merge",
    "post-rewrite",
    "pre-commit",
    "pre-merge-commit",
    "pre-push",
    "pre-rebase",
    "prepare-commit-msg",
    "manual",
];

/// `-V/--version` (`main.py:207-211`): `version=f'%(prog)s {C.VERSION}'`.
const VERSION_PC: ActionSpec = ActionSpec {
    nargs: Nargs::Zero,
    kind: Kind::Version,
    default: DefaultValue::Const("%(prog)s 4.6.2"),
    help: Some("show program's version number and exit"),
    ..store(&["-V", "--version"], "version")
};

/// `-V/--version` of the snp brand: `snp 2.0.0` (ADR-023).
const VERSION_SNP: ActionSpec = ActionSpec {
    default: DefaultValue::Const(concat!("%(prog)s ", env!("CARGO_PKG_VERSION"))),
    ..VERSION_PC
};

/// `add_color_option` (`color.py:103-109`): the raw `PRE_COMMIT_COLOR` string is the default (ADR-007).
pub(crate) const COLOR: ActionSpec = ActionSpec {
    conv: Conv::UseColor,
    default: DefaultValue::Env {
        var: "PRE_COMMIT_COLOR",
        fallback: "auto",
    },
    help: Some("Whether to use color in output.  Defaults to `%(default)s`."),
    metavar: Some("{auto,always,never}"),
    ..store(&["--color"], "color")
};

/// `_add_config_option` (`main.py:50-54`).
pub(crate) const CONFIG: ActionSpec = ActionSpec {
    default: DefaultValue::Str(".pre-commit-config.yaml"),
    help: Some("Path to alternate config file"),
    ..store(&["-c", "--config"], "config")
};

/// `_add_hook_type_option` (`main.py:57-61`).
pub(crate) const HOOK_TYPE: ActionSpec = ActionSpec {
    kind: Kind::Append,
    conv: Conv::HookType,
    choices: Some(HOOK_TYPES),
    ..store(&["-t", "--hook-type"], "hook_types")
};

// `_add_run_options` (main.py:64-172).
const HOOK: ActionSpec = positional("hook", Nargs::Optional, Some("A single hook-id to run"));
const VERBOSE: ActionSpec = store_true(&["--verbose", "-v"], "verbose", None);
const ALL_FILES: ActionSpec = ActionSpec {
    mutex_group: Some(0),
    ..store_true(
        &["--all-files", "-a"],
        "all_files",
        Some("Run on all the files in the repo."),
    )
};
const FILES: ActionSpec = ActionSpec {
    nargs: Nargs::ZeroOrMore,
    default: DefaultValue::EmptyList,
    help: Some("Specific filenames to run hooks on."),
    mutex_group: Some(0),
    ..store(&["--files"], "files")
};
const SHOW_DIFF: ActionSpec = store_true(
    &["--show-diff-on-failure"],
    "show_diff_on_failure",
    Some("When hooks fail, run `git diff` directly afterward."),
);
const FAIL_FAST: ActionSpec = store_true(
    &["--fail-fast"],
    "fail_fast",
    Some("Stop after the first failing hook."),
);
const HOOK_STAGE: ActionSpec = ActionSpec {
    conv: Conv::Stage,
    choices: Some(STAGES),
    default: DefaultValue::Str("pre-commit"),
    help: Some("The stage during which the hook is fired.  One of %(choices)s"),
    ..store(&["--hook-stage"], "hook_stage")
};
const REMOTE_BRANCH: ActionSpec = ActionSpec {
    help: Some("Remote branch ref used by `git push`."),
    ..store(&["--remote-branch"], "remote_branch")
};
const LOCAL_BRANCH: ActionSpec = ActionSpec {
    help: Some("Local branch ref used by `git push`."),
    ..store(&["--local-branch"], "local_branch")
};
const FROM_REF: ActionSpec = ActionSpec {
    help: Some(concat!(
        "(for usage with `--to-ref`) -- this option represents the ",
        "original ref in a `from_ref...to_ref` diff expression.  ",
        "For `pre-push` hooks, this represents the branch you are pushing ",
        "to.  ",
        "For `post-checkout` hooks, this represents the branch that was ",
        "previously checked out."
    )),
    ..store(&["--from-ref", "--source", "-s"], "from_ref")
};
const TO_REF: ActionSpec = ActionSpec {
    help: Some(concat!(
        "(for usage with `--from-ref`) -- this option represents the ",
        "destination ref in a `from_ref...to_ref` diff expression.  ",
        "For `pre-push` hooks, this represents the branch being pushed.  ",
        "For `post-checkout` hooks, this represents the branch that is ",
        "now checked out."
    )),
    ..store(&["--to-ref", "--origin", "-o"], "to_ref")
};
const PRE_REBASE_UPSTREAM: ActionSpec = ActionSpec {
    help: Some("The upstream from which the series was forked."),
    ..store(&["--pre-rebase-upstream"], "pre_rebase_upstream")
};
const PRE_REBASE_BRANCH: ActionSpec = ActionSpec {
    help: Some(concat!(
        "The branch being rebased, and is not set when  ",
        "rebasing the current branch."
    )),
    ..store(&["--pre-rebase-branch"], "pre_rebase_branch")
};
const COMMIT_MSG_FILENAME: ActionSpec = ActionSpec {
    help: Some("Filename to check when running during `commit-msg`"),
    ..store(&["--commit-msg-filename"], "commit_msg_filename")
};
const PREPARE_COMMIT_MESSAGE_SOURCE: ActionSpec = ActionSpec {
    help: Some(concat!(
        "Source of the commit message ",
        "(typically the second argument to .git/hooks/prepare-commit-msg)"
    )),
    ..store(
        &["--prepare-commit-message-source"],
        "prepare_commit_message_source",
    )
};
const COMMIT_OBJECT_NAME: ActionSpec = ActionSpec {
    help: Some(concat!(
        "Commit object name ",
        "(typically the third argument to .git/hooks/prepare-commit-msg)"
    )),
    ..store(&["--commit-object-name"], "commit_object_name")
};
const REMOTE_NAME: ActionSpec = ActionSpec {
    help: Some("Remote name used by `git push`."),
    ..store(&["--remote-name"], "remote_name")
};
const REMOTE_URL: ActionSpec = ActionSpec {
    help: Some("Remote url used by `git push`."),
    ..store(&["--remote-url"], "remote_url")
};
const CHECKOUT_TYPE: ActionSpec = ActionSpec {
    help: Some(concat!(
        "Indicates whether the checkout was a branch checkout ",
        "(changing branches, flag=1) or a file checkout (retrieving a ",
        "file from the index, flag=0)."
    )),
    ..store(&["--checkout-type"], "checkout_type")
};
const IS_SQUASH_MERGE: ActionSpec = ActionSpec {
    help: Some(concat!(
        "During a post-merge hook, indicates whether the merge was a ",
        "squash merge"
    )),
    ..store(&["--is-squash-merge"], "is_squash_merge")
};
const REWRITE_COMMAND: ActionSpec = ActionSpec {
    help: Some(concat!(
        "During a post-rewrite hook, specifies the command that invoked ",
        "the rewrite"
    )),
    ..store(&["--rewrite-command"], "rewrite_command")
};

/// `try-repo`'s `repo` and `--ref/--rev` (`main.py:318-329`).
const REPO: ActionSpec = positional("repo", Nargs::One, Some("Repository to source hooks from."));
const REF: ActionSpec = ActionSpec {
    help: Some(concat!(
        "Manually select a rev to run against, otherwise the `HEAD` ",
        "revision will be used."
    )),
    ..store(&["--ref", "--rev"], "ref")
};

// ----- the pre-commit 4.6.2 tree ------------------------------------------------------------------------------

/// `autoupdate` (`main.py:220-243`).
const AUTOUPDATE_ACTIONS: [ActionSpec; 6] = [
    COLOR,
    CONFIG,
    store_true(
        &["--bleeding-edge"],
        "bleeding_edge",
        Some(concat!(
            "Update to the bleeding edge of `HEAD` instead of the latest ",
            "tagged version (the default behavior)."
        )),
    ),
    store_true(
        &["--freeze"],
        "freeze",
        Some("Store \"frozen\" hashes in `rev` instead of tag names"),
    ),
    ActionSpec {
        kind: Kind::Append,
        default: DefaultValue::EmptyList,
        help: Some("Only update this repository -- may be specified multiple times."),
        metavar: Some("REPO"),
        ..store(&["--repo"], "repos")
    },
    ActionSpec {
        conv: Conv::Int,
        default: DefaultValue::Int(1),
        help: Some("Number of threads to use.  (default %(default)s)."),
        ..store(&["-j", "--jobs"], "jobs")
    },
];
static AUTOUPDATE: ParserSpec = parser(" autoupdate", &AUTOUPDATE_ACTIONS);

static COLOR_ONLY_ACTIONS: [ActionSpec; 1] = [COLOR];
static CLEAN: ParserSpec = parser(" clean", &COLOR_ONLY_ACTIONS);
static GC: ParserSpec = parser(" gc", &COLOR_ONLY_ACTIONS);
static SAMPLE_CONFIG: ParserSpec = parser(" sample-config", &COLOR_ONLY_ACTIONS);

/// `hazmat.add_parsers` (`commands/hazmat.py:10-28`).
static HAZMAT_CD: ParserSpec = parser(
    " cd",
    &[
        positional("subdir", Nargs::One, None),
        positional("cmd", Nargs::Remainder, None),
    ],
);
static HAZMAT_CMD_ONLY: [ActionSpec; 1] = [positional("cmd", Nargs::Remainder, None)];
static HAZMAT_IGNORE_EXIT_CODE: ParserSpec = parser(" ignore-exit-code", &HAZMAT_CMD_ONLY);
static HAZMAT_N1: ParserSpec = parser(" n1", &HAZMAT_CMD_ONLY);
static HAZMAT_TOOLS: [SubCommand; 3] = [
    cmd("cd", Some("cd to a subdir and run the command"), &HAZMAT_CD),
    cmd(
        "ignore-exit-code",
        Some("run the command but ignore the exit code"),
        &HAZMAT_IGNORE_EXIT_CODE,
    ),
    cmd("n1", Some("run the command once per filename"), &HAZMAT_N1),
];
static HAZMAT: ParserSpec = parser(
    " hazmat",
    &[COLOR, subparsers("tool", false, &HAZMAT_TOOLS)],
);

const DIRECTORY: ActionSpec = positional(
    "directory",
    Nargs::One,
    Some("The directory in which to write the hook script."),
);
const NO_ALLOW_MISSING_CONFIG: ActionSpec = ActionSpec {
    kind: Kind::StoreFalse,
    nargs: Nargs::Zero,
    default: DefaultValue::True,
    help: Some("Assume cloned repos should have a `pre-commit` config."),
    ..store(&["--no-allow-missing-config"], "allow_missing_config")
};
/// `init-templatedir` (`main.py:252-271`).
static INIT_TEMPLATEDIR: ParserSpec = parser(
    " init-templatedir",
    &[COLOR, CONFIG, DIRECTORY, NO_ALLOW_MISSING_CONFIG, HOOK_TYPE],
);

const OVERWRITE: ActionSpec = store_true(
    &["-f", "--overwrite"],
    "overwrite",
    Some("Overwrite existing hooks / remove migration mode."),
);
const INSTALL_HOOKS_FLAG: ActionSpec = store_true(
    &["--install-hooks"],
    "install_hooks",
    Some(concat!(
        "Whether to install hook environments for all environments ",
        "in the config file."
    )),
);
const ALLOW_MISSING_CONFIG: ActionSpec = store_true(
    &["--allow-missing-config"],
    "allow_missing_config",
    Some(concat!(
        "Whether to allow a missing `pre-commit` configuration file ",
        "or exit with a failure code."
    )),
);
/// `install` (`main.py:273-293`).
pub(crate) static INSTALL: ParserSpec = parser(
    " install",
    &[
        COLOR,
        CONFIG,
        OVERWRITE,
        INSTALL_HOOKS_FLAG,
        HOOK_TYPE,
        ALLOW_MISSING_CONFIG,
    ],
);

static COLOR_CONFIG_ACTIONS: [ActionSpec; 2] = [COLOR, CONFIG];
static INSTALL_HOOKS: ParserSpec = parser(" install-hooks", &COLOR_CONFIG_ACTIONS);
static MIGRATE_CONFIG: ParserSpec = parser(" migrate-config", &COLOR_CONFIG_ACTIONS);

/// The actions of a parser built with `_add_run_options` (`main.py:64-172`): `head` (`--color`, `-c`, …), the hook
/// positional, the `--all-files | --files` group plus `group` extras, pre-commit's run options, then `tail`.
macro_rules! run_parser_actions {
    ([$($head:expr),*], $hook:expr, [$($group:expr),*], [$($tail:expr),*]) => {
        [
            $($head,)*
            $hook,
            VERBOSE,
            ALL_FILES,
            FILES,
            $($group,)*
            SHOW_DIFF,
            FAIL_FAST,
            HOOK_STAGE,
            REMOTE_BRANCH,
            LOCAL_BRANCH,
            FROM_REF,
            TO_REF,
            PRE_REBASE_UPSTREAM,
            PRE_REBASE_BRANCH,
            COMMIT_MSG_FILENAME,
            PREPARE_COMMIT_MESSAGE_SOURCE,
            COMMIT_OBJECT_NAME,
            REMOTE_NAME,
            REMOTE_URL,
            CHECKOUT_TYPE,
            IS_SQUASH_MERGE,
            REWRITE_COMMAND
            $(, $tail)*
        ]
    };
}

/// `run` (`main.py:312-314`): `--color`, `-c`, then `_add_run_options`.
pub(crate) static RUN: ParserSpec =
    parser(" run", &run_parser_actions!([COLOR, CONFIG], HOOK, [], []));

/// `try-repo` (`main.py:316-330`).
static TRY_REPO: ParserSpec = parser(
    " try-repo",
    &run_parser_actions!([COLOR, CONFIG, REPO, REF], HOOK, [], []),
);

/// `uninstall` (`main.py:333-338`).
pub(crate) static UNINSTALL: ParserSpec = parser(" uninstall", &[COLOR, CONFIG, HOOK_TYPE]);

static FILENAMES_ACTIONS: [ActionSpec; 2] =
    [COLOR, positional("filenames", Nargs::ZeroOrMore, None)];
static VALIDATE_CONFIG: ParserSpec = parser(" validate-config", &FILENAMES_ACTIONS);
static VALIDATE_MANIFEST: ParserSpec = parser(" validate-manifest", &FILENAMES_ACTIONS);

/// `help` (`main.py:350-354`): no `--color`.
static HELP: ParserSpec = parser(
    " help",
    &[positional(
        "help_cmd",
        Nargs::Optional,
        Some("Command to show help for."),
    )],
);

/// `hook-impl` (`main.py:356-365`): hidden from the commands block (no `help=`).
static HOOK_IMPL: ParserSpec = parser(
    " hook-impl",
    &[
        COLOR,
        CONFIG,
        ActionSpec {
            required: true,
            ..store(&["--hook-type"], "hook_type")
        },
        store(&["--hook-dir"], "hook_dir"),
        store_true(
            &["--skip-on-missing-config"],
            "skip_on_missing_config",
            None,
        ),
        positional("rest", Nargs::Remainder, None),
    ],
);

const HELP_CMD_HELP: &str = "Show help for a specific command.";
const HAZMAT_HELP: &str = "Composable tools for rare use in hook `entry`.";
const INIT_TEMPLATEDIR_HELP: &str = concat!(
    "Install hook script in a directory intended for use with ",
    "`git config init.templateDir`."
);
const MIGRATE_CONFIG_HELP: &str = "Migrate list configuration to new map configuration.";
const TRY_REPO_HELP: &str = "Try the hooks in a repository, useful for developing new hooks.";
const SAMPLE_CONFIG_HELP: &str = "Produce a sample .pre-commit-config.yaml file";

/// `subparsers.add_parser(…)` in `main.py` order (`help` and `hook-impl` last).
static PC_COMMANDS: [SubCommand; 16] = [
    cmd(
        "autoupdate",
        Some("Auto-update pre-commit config to the latest repos' versions."),
        &AUTOUPDATE,
    ),
    cmd("clean", Some("Clean out pre-commit files."), &CLEAN),
    cmd("gc", Some("Clean unused cached repos."), &GC),
    cmd("hazmat", Some(HAZMAT_HELP), &HAZMAT),
    cmd(
        "init-templatedir",
        Some(INIT_TEMPLATEDIR_HELP),
        &INIT_TEMPLATEDIR,
    ),
    cmd("install", Some("Install the pre-commit script."), &INSTALL),
    cmd(
        "install-hooks",
        Some(concat!(
            "Install hook environments for all environments in the config ",
            "file.  You may find `pre-commit install --install-hooks` more ",
            "useful."
        )),
        &INSTALL_HOOKS,
    ),
    cmd("migrate-config", Some(MIGRATE_CONFIG_HELP), &MIGRATE_CONFIG),
    cmd("run", Some("Run hooks."), &RUN),
    cmd("sample-config", Some(SAMPLE_CONFIG_HELP), &SAMPLE_CONFIG),
    cmd("try-repo", Some(TRY_REPO_HELP), &TRY_REPO),
    cmd(
        "uninstall",
        Some("Uninstall the pre-commit script."),
        &UNINSTALL,
    ),
    cmd(
        "validate-config",
        Some("Validate .pre-commit-config.yaml files"),
        &VALIDATE_CONFIG,
    ),
    cmd(
        "validate-manifest",
        Some("Validate .pre-commit-hooks.yaml files"),
        &VALIDATE_MANIFEST,
    ),
    cmd("help", Some(HELP_CMD_HELP), &HELP),
    cmd("hook-impl", None, &HOOK_IMPL),
];

static PC_TOP: ParserSpec = parser(
    "",
    &[VERSION_PC, subparsers("command", false, &PC_COMMANDS)],
);

/// Compat brand tree (12 §A.8.1, verbatim help strings).
pub static PRE_COMMIT_SPEC: &ParserSpec = &PC_TOP;

// ----- the snp brand: compat ⊕ extensions (ARCHITECTURE §9.3, COMPATIBILITY §8.1) ---------------------------

/// Several hook ids or aliases (`:` allowed); replaces the single `hook` of pre-commit.
const SNP_HOOKS: ActionSpec = positional(
    "hook",
    Nargs::ZeroOrMore,
    Some("Hook ids or aliases to run (default: every hook of the stage)."),
);
/// `--stdin-files[=nul|lines]`, a third member of the `--all-files | --files` group.
const STDIN_FILES: ActionSpec = ActionSpec {
    nargs: Nargs::Optional,
    kind: Kind::StoreConst("lines"),
    choices: Some(&["nul", "lines"]),
    help: Some(concat!(
        "Run on the file names read from stdin: one per line (`lines`, the ",
        "default) or NUL-separated (`nul`)."
    )),
    mutex_group: Some(0),
    ..store(&["--stdin-files"], "stdin_files")
};
const JOBS: ActionSpec = ActionSpec {
    conv: Conv::Int,
    help: Some("Number of hooks to run in parallel (default: the number of CPUs)."),
    ..store(&["-j", "--jobs"], "jobs")
};
const GROUP: ActionSpec = ActionSpec {
    kind: Kind::Append,
    metavar: Some("GROUP"),
    help: Some(concat!(
        "Run only hooks in this group; repeat to select hooks in any of the ",
        "groups. `@ungrouped` selects hooks without groups."
    )),
    ..store(&["--group"], "groups")
};
const REQUIRE_GROUP: ActionSpec = ActionSpec {
    kind: Kind::Append,
    metavar: Some("GROUP"),
    help: Some("Run only hooks in this group; repeat to require every group."),
    ..store(&["--require-group"], "require_groups")
};
const NO_GROUP: ActionSpec = ActionSpec {
    kind: Kind::Append,
    metavar: Some("GROUP"),
    help: Some(concat!(
        "Skip hooks in this group (repeatable; wins over --group and ",
        "--require-group)."
    )),
    ..store(&["--no-group"], "no_groups")
};
const NO_FAST_PATH: ActionSpec = store_true(
    &["--no-fast-path"],
    "no_fast_path",
    Some(concat!(
        "Run pre-commit-hooks in their Python environments instead of ",
        "snp's native implementations."
    )),
);
const SUMMARY: ActionSpec = store_true(
    &["--summary"],
    "summary",
    Some("Print a summary of the hook results at the end."),
);
const NO_LOCAL_CONFIG: ActionSpec = store_true(
    &["--no-local-config"],
    "no_local_config",
    Some("Ignore the local override file (.pre-commit-config.local.yaml)."),
);

/// `snp run`: pre-commit's options with several hook ids, `--stdin-files` in the file-selection group and the
/// extension flags after `--rewrite-command`.
pub(crate) static SNP_RUN: ParserSpec = parser(
    " run",
    &run_parser_actions!(
        [COLOR, CONFIG],
        SNP_HOOKS,
        [STDIN_FILES],
        [
            JOBS,
            GROUP,
            REQUIRE_GROUP,
            NO_GROUP,
            NO_FAST_PATH,
            SUMMARY,
            NO_LOCAL_CONFIG
        ]
    ),
);

static SNP_TRY_REPO: ParserSpec = parser(
    " try-repo",
    &run_parser_actions!(
        [COLOR, CONFIG, REPO, REF],
        SNP_HOOKS,
        [STDIN_FILES],
        [
            JOBS,
            GROUP,
            REQUIRE_GROUP,
            NO_GROUP,
            NO_FAST_PATH,
            SUMMARY,
            NO_LOCAL_CONFIG
        ]
    ),
);

/// `--git-config-hooks` (ADR-057; `--config-hooks` would make `--conf` ambiguous).
const GIT_CONFIG_HOOKS_INSTALL: ActionSpec = store_true(
    &["--git-config-hooks"],
    "git_config_hooks",
    Some(concat!(
        "Install the hooks as git config entries (`hook.<name>.command`, git ",
        "2.54+) instead of hook scripts."
    )),
);
const GIT_CONFIG_HOOKS_UNINSTALL: ActionSpec = store_true(
    &["--git-config-hooks"],
    "git_config_hooks",
    Some("Remove the git config hook entries instead of hook scripts."),
);
/// The `install` help strings that name the tool (COMPATIBILITY §3.3, `main.py:290`).
const SNP_ALLOW_MISSING_CONFIG: ActionSpec = ActionSpec {
    help: Some(concat!(
        "Whether to allow a missing `snp` configuration file ",
        "or exit with a failure code."
    )),
    ..ALLOW_MISSING_CONFIG
};
pub(crate) static SNP_INSTALL: ParserSpec = parser(
    " install",
    &[
        COLOR,
        CONFIG,
        OVERWRITE,
        INSTALL_HOOKS_FLAG,
        HOOK_TYPE,
        SNP_ALLOW_MISSING_CONFIG,
        GIT_CONFIG_HOOKS_INSTALL,
    ],
);
pub(crate) static SNP_UNINSTALL: ParserSpec = parser(
    " uninstall",
    &[COLOR, CONFIG, HOOK_TYPE, GIT_CONFIG_HOOKS_UNINSTALL],
);
static SNP_INSTALL_HOOKS: ParserSpec = parser(
    " install-hooks",
    &[
        COLOR,
        CONFIG,
        store_true(
            &["--with-fallback"],
            "with_fallback",
            Some(concat!(
                "Also install the Python environments that hooks run natively ",
                "by snp would fall back to (for offline use with ",
                "SNP_NO_FAST_PATH)."
            )),
        ),
    ],
);

const STRICT: ActionSpec = store_true(
    &["--strict"],
    "strict",
    Some("Also report unknown hook keys (pre-commit only warns when it runs the hook)."),
);
const PORTABLE: ActionSpec = store_true(
    &["--portable"],
    "portable",
    Some("Also list the snp extension keys that pre-commit would warn about."),
);
static SNP_FILENAMES_ACTIONS: [ActionSpec; 4] = [
    COLOR,
    positional("filenames", Nargs::ZeroOrMore, None),
    STRICT,
    PORTABLE,
];
static SNP_VALIDATE_CONFIG: ParserSpec = parser(" validate-config", &SNP_FILENAMES_ACTIONS);
static SNP_VALIDATE_MANIFEST: ParserSpec = parser(" validate-manifest", &SNP_FILENAMES_ACTIONS);

// snp-only commands.

const JSON: ActionSpec = store_true(&["--json"], "json", Some("Print JSON instead of text."));
const LIST_STAGE: ActionSpec = ActionSpec {
    default: DefaultValue::None,
    help: Some("Only list the hooks of this stage.  One of %(choices)s"),
    ..HOOK_STAGE
};
/// `snp list`.
static LIST: ParserSpec = parser(
    " list",
    &[CONFIG, JSON, LIST_STAGE, GROUP, REQUIRE_GROUP, NO_GROUP],
);

/// `snp config dump|schema`.
static CONFIG_DUMP: ParserSpec = parser(
    " dump",
    &[
        CONFIG,
        store_true(&["--json"], "json", Some("Print JSON instead of YAML.")),
        store_true(
            &["--no-local-config"],
            "no_local_config",
            Some("Ignore the local override file (.pre-commit-config.local.yaml)."),
        ),
    ],
);
static CONFIG_SCHEMA: ParserSpec = parser(" schema", &[]);
static CONFIG_COMMANDS: [SubCommand; 2] = [
    cmd(
        "dump",
        Some("Print the merged configuration, local overrides included."),
        &CONFIG_DUMP,
    ),
    cmd(
        "schema",
        Some("Print the JSON schema of .pre-commit-config.yaml."),
        &CONFIG_SCHEMA,
    ),
];
static CONFIG_CMD: ParserSpec = parser(
    " config",
    &[subparsers("config_cmd", true, &CONFIG_COMMANDS)],
);

/// `snp cache dir|size|gc|clean`.
static CACHE_DIR: ParserSpec = parser(" dir", &[]);
static CACHE_SIZE: ParserSpec = parser(" size", &[]);
static CACHE_GC: ParserSpec = parser(" gc", &[]);
static CACHE_CLEAN: ParserSpec = parser(" clean", &[]);
static CACHE_COMMANDS: [SubCommand; 4] = [
    cmd("dir", Some("Print the cache directory."), &CACHE_DIR),
    cmd("size", Some("Print the size of the cache."), &CACHE_SIZE),
    cmd(
        "gc",
        Some("Remove unused repositories and environments."),
        &CACHE_GC,
    ),
    cmd("clean", Some("Remove the whole cache."), &CACHE_CLEAN),
];
static CACHE: ParserSpec = parser(" cache", &[subparsers("cache_cmd", true, &CACHE_COMMANDS)]);

/// `snp migrate lefthook|builtin|upstream`.
static MIGRATE_LEFTHOOK: ParserSpec = parser(
    " lefthook",
    &[
        ActionSpec {
            help: Some("Repository root (default: the current git repository)."),
            metavar: Some("DIR"),
            ..store(&["--root"], "root")
        },
        ActionSpec {
            help: Some("lefthook configuration to convert (default: lefthook's lookup)."),
            metavar: Some("FILE"),
            ..store(&["-c", "--config"], "config")
        },
        ActionSpec {
            help: Some(
                "Where to write the result (default: .snp.yaml, .pre-commit-config.yaml with \
                 --portable; `-` for stdout).",
            ),
            metavar: Some("FILE"),
            ..store(&["-o", "--output"], "output")
        },
        store_true(
            &["--portable"],
            "portable",
            Some("Emit only keys pre-commit 4.x understands."),
        ),
        store_true(
            &["--local"],
            "local",
            Some(
                "Also convert lefthook-local.yml into the local overlay beside the output \
                 (.snp.local.yaml, .pre-commit-config.local.yaml with --portable).",
            ),
        ),
        store_true(
            &["--strict"],
            "strict",
            Some("Fail (exit 2) when a feature cannot be converted faithfully."),
        ),
        store_true(
            &["--install"],
            "install",
            Some("Afterwards remove lefthook's hooks and install snp's."),
        ),
        store_true(
            &["-f", "--force"],
            "force",
            Some("Overwrite an existing output file."),
        ),
    ],
);
static MIGRATE_CONFIG_ONLY: [ActionSpec; 1] = [CONFIG];
static MIGRATE_BUILTIN: ParserSpec = parser(" builtin", &MIGRATE_CONFIG_ONLY);
static MIGRATE_UPSTREAM: ParserSpec = parser(" upstream", &MIGRATE_CONFIG_ONLY);
static MIGRATE_COMMANDS: [SubCommand; 3] = [
    cmd(
        "lefthook",
        Some("Convert a lefthook configuration."),
        &MIGRATE_LEFTHOOK,
    ),
    cmd(
        "builtin",
        Some("Switch pre-commit-hooks repos to `repo: builtin`."),
        &MIGRATE_BUILTIN,
    ),
    cmd(
        "upstream",
        Some("Switch `repo: builtin` back to pre-commit-hooks."),
        &MIGRATE_UPSTREAM,
    ),
];
static MIGRATE: ParserSpec = parser(
    " migrate",
    &[subparsers("migrate_cmd", true, &MIGRATE_COMMANDS)],
);

/// `snp doctor`.
static DOCTOR: ParserSpec = parser(
    " doctor",
    &[
        CONFIG,
        store_true(
            &["--fix"],
            "fix",
            Some("Repair this repository's hook scripts, then check again."),
        ),
        store_true(
            &["--fast-path"],
            "fast_path",
            Some("Explain the native fast-path decision of every hook."),
        ),
        store_true(
            &["--stash"],
            "stash",
            Some("Check orphaned stashes against the worktree; with --fix, dismiss their notices."),
        ),
    ],
);

/// `snp self alias install|uninstall|status` and `snp self check`.
static ALIAS_INSTALL: ParserSpec = parser(
    " install",
    &[
        ActionSpec {
            help: Some("Directory of the alias (default: the directory of snp)."),
            metavar: Some("DIR"),
            ..store(&["--dir"], "dir")
        },
        store_true(
            &["-f", "--force"],
            "force",
            Some("Replace an existing pre-commit, keeping it as pre-commit.snp-backup."),
        ),
    ],
);
static ALIAS_UNINSTALL: ParserSpec = parser(" uninstall", &[]);
static ALIAS_STATUS: ParserSpec = parser(" status", &[]);
static ALIAS_COMMANDS: [SubCommand; 3] = [
    cmd(
        "install",
        Some("Create a `pre-commit` alias of snp."),
        &ALIAS_INSTALL,
    ),
    cmd(
        "uninstall",
        Some("Remove the `pre-commit` alias."),
        &ALIAS_UNINSTALL,
    ),
    cmd(
        "status",
        Some("Show what `pre-commit` resolves to."),
        &ALIAS_STATUS,
    ),
];
static SELF_ALIAS: ParserSpec = parser(" alias", &[subparsers("alias_cmd", true, &ALIAS_COMMANDS)]);
static SELF_CHECK: ParserSpec = parser(" check", &[]);
static SELF_COMMANDS: [SubCommand; 2] = [
    cmd("alias", Some("Manage the `pre-commit` alias."), &SELF_ALIAS),
    cmd("check", Some("Run the built-in self-test."), &SELF_CHECK),
];
static SELF: ParserSpec = parser(" self", &[subparsers("self_cmd", true, &SELF_COMMANDS)]);

/// `snp completions SHELL`.
static COMPLETIONS_CMD: ParserSpec = parser(
    " completions",
    &[ActionSpec {
        choices: Some(&["bash", "zsh", "fish", "powershell"]),
        help: Some("The shell to print the completion script for."),
        ..positional("shell", Nargs::One, None)
    }],
);

/// The snp brand's commands: pre-commit's (help strings naming the tool substituted, COMPATIBILITY §3.3) with the
/// snp-only commands inserted alphabetically; `help` and `hook-impl` stay last.
static SNP_COMMANDS: [SubCommand; 23] = [
    cmd(
        "autoupdate",
        Some("Auto-update snp config to the latest repos' versions."),
        &AUTOUPDATE,
    ),
    cmd("cache", Some("Inspect or clean snp's cache."), &CACHE),
    cmd("clean", Some("Clean out snp files."), &CLEAN),
    cmd(
        "completions",
        Some("Print a shell completion script."),
        &COMPLETIONS_CMD,
    ),
    cmd(
        "config",
        Some("Print the merged configuration or its JSON schema."),
        &CONFIG_CMD,
    ),
    cmd(
        "doctor",
        Some("Check snp, the hook scripts and the cache for problems."),
        &DOCTOR,
    ),
    cmd("gc", Some("Clean unused cached repos."), &GC),
    cmd("hazmat", Some(HAZMAT_HELP), &HAZMAT),
    cmd(
        "init-templatedir",
        Some(INIT_TEMPLATEDIR_HELP),
        &INIT_TEMPLATEDIR,
    ),
    cmd("install", Some("Install the snp script."), &SNP_INSTALL),
    cmd(
        "install-hooks",
        Some(concat!(
            "Install hook environments for all environments in the config ",
            "file.  You may find `snp install --install-hooks` more ",
            "useful."
        )),
        &SNP_INSTALL_HOOKS,
    ),
    cmd("list", Some("List the hooks of the configuration."), &LIST),
    cmd(
        "migrate",
        Some(
            "Convert lefthook configurations or switch pre-commit-hooks to and from `repo: builtin`.",
        ),
        &MIGRATE,
    ),
    cmd("migrate-config", Some(MIGRATE_CONFIG_HELP), &MIGRATE_CONFIG),
    cmd("run", Some("Run hooks."), &SNP_RUN),
    cmd("sample-config", Some(SAMPLE_CONFIG_HELP), &SAMPLE_CONFIG),
    cmd("self", Some("Manage the snp installation."), &SELF),
    cmd("try-repo", Some(TRY_REPO_HELP), &SNP_TRY_REPO),
    cmd(
        "uninstall",
        Some("Uninstall the snp script."),
        &SNP_UNINSTALL,
    ),
    cmd(
        "validate-config",
        Some("Validate .pre-commit-config.yaml files"),
        &SNP_VALIDATE_CONFIG,
    ),
    cmd(
        "validate-manifest",
        Some("Validate .pre-commit-hooks.yaml files"),
        &SNP_VALIDATE_MANIFEST,
    ),
    cmd("help", Some(HELP_CMD_HELP), &HELP),
    cmd("hook-impl", None, &HOOK_IMPL),
];

static SNP_TOP: ParserSpec = parser(
    "",
    &[VERSION_SNP, subparsers("command", false, &SNP_COMMANDS)],
);

/// snp brand tree = compat ⊕ extensions (prefix-stable, 12 §A.17 item 6).
pub static SNP_SPEC: &ParserSpec = &SNP_TOP;

/// Shell completion scripts, generated by `xtask completions` from [`SNP_SPEC`] (`cargo xtask completions`
/// rewrites `crates/snp-cli/completions/`; `snp completions <shell>` prints them).
pub static COMPLETIONS: &[(Shell, &str)] = &[
    (Shell::Bash, include_str!("../completions/snp.bash")),
    (Shell::Zsh, include_str!("../completions/_snp")),
    (Shell::Fish, include_str!("../completions/snp.fish")),
    (Shell::PowerShell, include_str!("../completions/_snp.ps1")),
];

/// Planted options of the prefix-stability acceptance (ARCHITECTURE §9.2, ADR-057): each would break an
/// abbreviation pre-commit users rely on (`--fi`/`--fil`/`--file`, `--con`/`--conf`/`--confi`).
#[cfg(test)]
pub(crate) mod planted {
    use super::*;

    const FILES_FROM_STDIN: ActionSpec =
        store_true(&["--files-from-stdin"], "files_from_stdin", None);
    const CONFIG_HOOKS: ActionSpec = store_true(&["--config-hooks"], "config_hooks", None);

    /// `snp run` plus `--files-from-stdin`.
    pub(crate) static RUN_FILES_FROM_STDIN: ParserSpec = parser(
        " run",
        &run_parser_actions!(
            [COLOR, CONFIG],
            SNP_HOOKS,
            [STDIN_FILES],
            [
                JOBS,
                GROUP,
                REQUIRE_GROUP,
                NO_GROUP,
                NO_FAST_PATH,
                SUMMARY,
                NO_LOCAL_CONFIG,
                FILES_FROM_STDIN
            ]
        ),
    );

    /// `snp install` with `--config-hooks` instead of `--git-config-hooks`.
    pub(crate) static INSTALL_CONFIG_HOOKS: ParserSpec = parser(
        " install",
        &[
            COLOR,
            CONFIG,
            OVERWRITE,
            INSTALL_HOOKS_FLAG,
            HOOK_TYPE,
            SNP_ALLOW_MISSING_CONFIG,
            CONFIG_HOOKS,
        ],
    );

    /// `snp uninstall` with `--config-hooks`.
    pub(crate) static UNINSTALL_CONFIG_HOOKS: ParserSpec =
        parser(" uninstall", &[COLOR, CONFIG, HOOK_TYPE, CONFIG_HOOKS]);
}
