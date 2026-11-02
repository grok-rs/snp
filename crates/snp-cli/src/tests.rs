//! Unit tests of the typed facade, the brand split and the prefix-stability guard (WP-12 acceptance). The byte
//! goldens of every help/usage/error text live in `tests/goldens.rs`.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;

use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::output::ColorSetting;
use snp_core::stage::{HookType, Stage};

use crate::specs::{self, planted};
use crate::{
    CacheAction, Cli, CliOutcome, ConfigArg, ConfigCmd, HazmatTool, MigrateArgs, SelfArgs, Shell,
    StdinFormat, parse,
};

fn argv(a: &[&str]) -> Vec<OsString> {
    a.iter().map(OsString::from).collect()
}

fn run_env(brand: Brand, a: &[&str], env: &[(&str, &str)]) -> CliOutcome {
    let env = Env::from_pairs(env.iter().copied());
    parse(brand, &argv(a), &env, None)
}

fn run(brand: Brand, a: &[&str]) -> CliOutcome {
    run_env(brand, a, &[])
}

fn cli(brand: Brand, a: &[&str]) -> Cli {
    match run(brand, a) {
        CliOutcome::Cli(c) => c,
        other => panic!("{a:?}: {other:?}"),
    }
}

fn usage(brand: Brand, a: &[&str]) -> String {
    match run(brand, a) {
        CliOutcome::Usage(s) => s,
        other => panic!("{a:?}: {other:?}"),
    }
}

fn last_line(s: &str) -> &str {
    s.lines().last().unwrap()
}

const BOTH: [Brand; 2] = [Brand::PreCommit, Brand::Snp];

#[test]
fn empty_argv_is_run() {
    for brand in BOTH {
        let Cli::Run(r) = cli(brand, &[]) else {
            panic!()
        };
        assert_eq!(r.color, ColorSetting::Auto);
        assert_eq!(r.config, ConfigArg::absent());
        assert_eq!(r.config.path, ".pre-commit-config.yaml");
        assert_eq!(r.config.explicit(), None);
        assert_eq!(r.hook_stage, Stage::Hook(HookType::PreCommit));
        assert!(r.hooks.is_empty() && r.files.is_empty() && !r.all_files);
    }
}

#[test]
fn run_values() {
    let Cli::Run(r) = cli(
        Brand::PreCommit,
        &[
            "run",
            "lint:js",
            "--color=never",
            "-cx.yaml",
            "-av",
            "--hook-stage",
            "push",
            "--from-ref",
            "",
            "--to-ref",
            "t",
            "--commit-msg-filename",
            "",
            "--remote-branch",
            "rb",
            "--show-diff",
            "--fail-fast",
        ],
    ) else {
        panic!()
    };
    assert_eq!(r.hooks, argv(&["lint:js"]));
    assert_eq!(r.color, ColorSetting::Never);
    assert_eq!(r.config, ConfigArg::given("x.yaml"));
    assert!(r.all_files && r.verbose && r.show_diff_on_failure && r.fail_fast);
    assert_eq!(r.hook_stage, Stage::Hook(HookType::PrePush));
    // `Some("")` ref options are falsy in pre-commit; `--commit-msg-filename` is tested with `is not None`.
    assert_eq!(r.from_ref, None);
    assert_eq!(r.to_ref, Some("t".into()));
    assert_eq!(r.commit_msg_filename, Some(OsString::new()));
    assert_eq!(r.remote_branch, Some("rb".into()));
    assert!(r.ext.groups.is_empty() && r.ext.jobs.is_none() && r.ext.stdin_files.is_none());
}

#[test]
fn empty_hook_id_means_all_hooks() {
    let Cli::Run(r) = cli(Brand::PreCommit, &["run", ""]) else {
        panic!()
    };
    assert_eq!(r.hooks, argv(&[]));
}

#[test]
fn colon_hook_ids_in_both_brands() {
    for brand in BOTH {
        let Cli::Run(r) = cli(brand, &["run", "lint:js"]) else {
            panic!()
        };
        assert_eq!(r.hooks, argv(&["lint:js"]));
    }
    let Cli::TryRepo(t) = cli(Brand::Snp, &["try-repo", "r", "a:b", "c"]) else {
        panic!()
    };
    assert_eq!(t.run.hooks, argv(&["a:b", "c"]));
}

#[test]
fn snp_run_extensions() {
    let Cli::Run(r) = cli(
        Brand::Snp,
        &[
            "run",
            "a",
            "b",
            "-j",
            "4",
            "--group",
            "g1",
            "--group",
            "@ungrouped",
            "--require-group",
            "r",
            "--no-group",
            "n",
            "--no-fast-path",
            "--stdin-files=nul",
            "--summary",
            "--no-local-config",
        ],
    ) else {
        panic!()
    };
    assert_eq!(r.hooks, argv(&["a", "b"]));
    assert_eq!(r.ext.jobs, NonZeroUsize::new(4));
    assert_eq!(r.ext.groups, argv(&["g1", "@ungrouped"]));
    assert_eq!(r.ext.require_groups, argv(&["r"]));
    assert_eq!(r.ext.no_groups, argv(&["n"]));
    assert!(r.ext.no_fast_path && r.ext.summary && r.ext.no_local_config);
    assert_eq!(r.ext.stdin_files, Some(StdinFormat::Nul));

    let Cli::Run(r) = cli(Brand::Snp, &["run", "--stdin-files", "-j0"]) else {
        panic!()
    };
    assert_eq!(r.ext.stdin_files, Some(StdinFormat::Lines));
    assert_eq!(r.ext.jobs, None);
    // `--stdin-files` joins the `--all-files | --files` group.
    assert_eq!(
        last_line(&usage(Brand::Snp, &["run", "-a", "--stdin-files"])),
        "snp run: error: argument --stdin-files: not allowed with argument --all-files/-a"
    );
}

#[test]
fn compat_brand_rejects_snp_only_surface() {
    let choices = "(choose from 'autoupdate', 'clean', 'gc', 'hazmat', 'init-templatedir', 'install', \
                   'install-hooks', 'migrate-config', 'run', 'sample-config', 'try-repo', 'uninstall', \
                   'validate-config', 'validate-manifest', 'help', 'hook-impl')";
    for cmd in [
        "list",
        "config",
        "cache",
        "migrate",
        "doctor",
        "self",
        "completions",
    ] {
        assert_eq!(
            last_line(&usage(Brand::PreCommit, &[cmd])),
            format!("pre-commit: error: argument command: invalid choice: '{cmd}' {choices}")
        );
        // ... and the snp brand accepts them (help shown or a required sub-command missing).
        let o = run(Brand::Snp, &[cmd, "-h"]);
        assert!(matches!(o, CliOutcome::Stdout(_)), "{cmd}: {o:?}");
    }
    for (a, token) in [
        // The value after an unknown option becomes the `hook` positional, as in argparse.
        (&["run", "-j", "2"][..], "-j"),
        (&["run", "--group", "g"][..], "--group"),
        (
            &["run", "--require-group", "g", "x"][..],
            "--require-group x",
        ),
        (&["run", "--no-group=g"][..], "--no-group=g"),
        (&["run", "--no-fast-path"][..], "--no-fast-path"),
        (&["run", "--stdin-files"][..], "--stdin-files"),
        (&["run", "--summary"][..], "--summary"),
        (&["run", "--no-local-config"][..], "--no-local-config"),
        (&["run", "a", "b"][..], "b"),
        (&["install", "--git-config-hooks"][..], "--git-config-hooks"),
        (
            &["uninstall", "--git-config-hooks"][..],
            "--git-config-hooks",
        ),
        (&["install-hooks", "--with-fallback"][..], "--with-fallback"),
        (&["validate-config", "--strict"][..], "--strict"),
        (&["validate-manifest", "--portable"][..], "--portable"),
    ] {
        let u = usage(Brand::PreCommit, a);
        assert!(u.starts_with("usage: pre-commit [-h] [-V]\n"), "{a:?}");
        assert_eq!(
            last_line(&u),
            format!("pre-commit: error: unrecognized arguments: {token}"),
            "{a:?}"
        );
    }
}

#[test]
fn install_conf_abbreviation_still_means_config() {
    for brand in BOTH {
        let Cli::Install(i) = cli(brand, &["install", "--conf", "x.yaml"]) else {
            panic!()
        };
        assert_eq!(i.config, ConfigArg::given("x.yaml"));
        assert!(!i.git_config_hooks);
        let Cli::Uninstall(u) = cli(brand, &["uninstall", "--con", "y.yaml"]) else {
            panic!()
        };
        assert_eq!(u.config, ConfigArg::given("y.yaml"));
    }
    let Cli::Install(i) = cli(
        Brand::Snp,
        &["install", "--git", "-t", "pre-push", "-tcommit-msg", "-f"],
    ) else {
        panic!()
    };
    assert!(i.git_config_hooks && i.overwrite);
    assert_eq!(i.hook_types, [HookType::PrePush, HookType::CommitMsg]);
}

#[test]
fn prefix_stability() {
    snp_argparse::prefix_stable(specs::PRE_COMMIT_SPEC, specs::SNP_SPEC).unwrap();
    snp_argparse::prefix_stable(&specs::RUN, &specs::SNP_RUN).unwrap();
    let e = snp_argparse::prefix_stable(&specs::RUN, &planted::RUN_FILES_FROM_STDIN).unwrap_err();
    assert!(e.contains("`--fi`") && e.contains("--files"), "{e}");
    let e =
        snp_argparse::prefix_stable(&specs::INSTALL, &planted::INSTALL_CONFIG_HOOKS).unwrap_err();
    assert!(e.contains("`--con`") && e.contains("--config"), "{e}");
    let e = snp_argparse::prefix_stable(&specs::UNINSTALL, &planted::UNINSTALL_CONFIG_HOOKS)
        .unwrap_err();
    assert!(e.contains("`--con`"), "{e}");
    // The reverse direction loses commands and options.
    assert!(snp_argparse::prefix_stable(specs::SNP_SPEC, specs::PRE_COMMIT_SPEC).is_err());
}

#[test]
fn hook_impl_keeps_the_raw_remainder() {
    for brand in BOTH {
        let Cli::HookImpl(h) = cli(
            brand,
            &[
                "hook-impl",
                "--config=.pre-commit-config.yaml",
                "--hook-type=pre-push",
                "--hook-dir",
                "/r/.git/hooks",
                "--skip-on-missing-config",
                "--",
                "origin",
                "-h",
            ],
        ) else {
            panic!()
        };
        assert_eq!(h.hook_type, "pre-push");
        assert_eq!(h.hook_dir, Some("/r/.git/hooks".into()));
        assert!(h.skip_on_missing_config);
        assert_eq!(h.rest, argv(&["--", "origin", "-h"]));
        let Cli::HookImpl(h) = cli(
            brand,
            &["hook-impl", "--hook-type=x", "a", "--hook-dir", "y"],
        ) else {
            panic!()
        };
        assert_eq!(h.rest, argv(&["a", "--hook-dir", "y"]));
        assert_eq!(h.hook_dir, None);
    }
}

#[test]
fn hazmat_tools() {
    let Cli::Hazmat(h) = cli(Brand::PreCommit, &["hazmat", "cd", "sub", "-h"]) else {
        panic!()
    };
    let HazmatTool::Cd { subdir, cmd } = h.tool else {
        panic!()
    };
    assert_eq!(subdir, "sub");
    assert_eq!(cmd, argv(&["-h"]));
    let Cli::Hazmat(h) = cli(
        Brand::Snp,
        &["hazmat", "--color", "never", "n1", "--", "echo", "--", "a"],
    ) else {
        panic!()
    };
    assert_eq!(h.color, ColorSetting::Never);
    let HazmatTool::N1 { cmd } = h.tool else {
        panic!()
    };
    assert_eq!(cmd, argv(&["--", "echo", "--", "a"]));
    let Cli::Hazmat(h) = cli(Brand::PreCommit, &["hazmat", "ignore-exit-code", "false"]) else {
        panic!()
    };
    assert!(matches!(h.tool, HazmatTool::IgnoreExitCode { .. }));
}

#[test]
fn hazmat_without_tool_is_a_usage_error() {
    // D-CRASH-22: pre-commit crashes with `AttributeError` (exit 3); snp reports the missing tool, exit 2.
    assert_eq!(
        usage(Brand::PreCommit, &["hazmat"]),
        "usage: pre-commit hazmat [-h] [--color {auto,always,never}]\n                         \
         {cd,ignore-exit-code,n1} ...\npre-commit hazmat: error: the following arguments are required: tool\n"
    );
    assert_eq!(
        last_line(&usage(Brand::Snp, &["hazmat", "--color", "never"])),
        "snp hazmat: error: the following arguments are required: tool"
    );
    // Errors raised during the parse keep their precedence.
    let u = match run_env(
        Brand::PreCommit,
        &["hazmat"],
        &[("PRE_COMMIT_COLOR", "bogus")],
    ) {
        CliOutcome::Usage(u) => u,
        o => panic!("{o:?}"),
    };
    assert_eq!(
        last_line(&u),
        "pre-commit hazmat: error: argument --color: invalid use_color value: 'bogus'"
    );
}

#[test]
fn other_commands() {
    let brand = Brand::PreCommit;
    let Cli::Autoupdate(au) = cli(
        brand,
        &["autoupdate", "-j", " 2", "--repo", "x", "--freeze"],
    ) else {
        panic!()
    };
    assert_eq!((au.jobs, au.freeze, au.bleeding_edge), (2, true, false));
    assert_eq!(au.repos, argv(&["x"]));
    let Cli::Autoupdate(au) = cli(brand, &["autoupdate"]) else {
        panic!()
    };
    assert_eq!(au.jobs, 1);
    let Cli::InitTemplatedir(itd) = cli(
        brand,
        &["init-templatedir", "d", "--no-allow-missing-config"],
    ) else {
        panic!()
    };
    assert!(itd.no_allow_missing_config);
    assert_eq!(itd.directory, "d");
    let Cli::InitTemplatedir(itd) = cli(brand, &["init-templatedir", "d"]) else {
        panic!()
    };
    assert!(!itd.no_allow_missing_config && itd.hook_types.is_empty());
    let Cli::TryRepo(tr) = cli(brand, &["try-repo", "r", "--rev", "", "h"]) else {
        panic!()
    };
    assert_eq!(tr.rev, Some(OsString::new()));
    assert_eq!(tr.run.hooks, argv(&["h"]));
    let Cli::ValidateConfig(vc) = cli(brand, &["validate-config", "a", "b"]) else {
        panic!()
    };
    assert_eq!(vc.filenames, argv(&["a", "b"]));
    assert!(matches!(
        cli(brand, &["validate-manifest"]),
        Cli::ValidateManifest(_)
    ));
    assert!(matches!(cli(brand, &["clean"]), Cli::Clean(_)));
    assert!(matches!(cli(brand, &["gc"]), Cli::Gc(_)));
    assert!(matches!(
        cli(brand, &["sample-config"]),
        Cli::SampleConfig(_)
    ));
    assert!(matches!(
        cli(brand, &["migrate-config"]),
        Cli::MigrateConfig(_)
    ));
    let Cli::InstallHooks(ih) = cli(Brand::Snp, &["install-hooks", "--with-fallback"]) else {
        panic!()
    };
    assert!(ih.with_fallback);
    let Cli::ValidateConfig(vc) = cli(Brand::Snp, &["validate-config", "--strict", "--portable"])
    else {
        panic!()
    };
    assert!(vc.strict && vc.portable);
}

#[test]
fn snp_only_commands() {
    let s = Brand::Snp;
    let Cli::List(l) = cli(
        s,
        &["list", "--json", "--hook-stage", "commit", "--group", "g"],
    ) else {
        panic!()
    };
    assert!(l.json);
    assert_eq!(l.stage, Some(Stage::Hook(HookType::PreCommit)));
    assert_eq!(l.groups, argv(&["g"]));
    let Cli::List(l) = cli(s, &["list"]) else {
        panic!()
    };
    assert_eq!(l.stage, None);
    assert!(matches!(
        cli(s, &["config", "dump", "--json"]),
        Cli::Config(ConfigCmd::Dump { json: true, .. })
    ));
    assert!(matches!(
        cli(s, &["config", "schema"]),
        Cli::Config(ConfigCmd::Schema)
    ));
    for (name, action) in [
        ("dir", CacheAction::Dir),
        ("size", CacheAction::Size),
        ("gc", CacheAction::Gc),
        ("clean", CacheAction::Clean),
    ] {
        let Cli::Cache(c) = cli(s, &["cache", name]) else {
            panic!()
        };
        assert_eq!(format!("{:?}", c.action), format!("{action:?}"));
    }
    let Cli::Migrate(MigrateArgs::Lefthook {
        root,
        config,
        output,
        portable,
        install,
        force,
        ..
    }) = cli(
        s,
        &[
            "migrate",
            "lefthook",
            "--root",
            "r",
            "-o",
            "-",
            "--portable",
            "--install",
            "-f",
        ],
    )
    else {
        panic!()
    };
    assert_eq!(
        (root, config, output),
        (Some("r".into()), None, Some("-".into()))
    );
    assert!(portable && install && force);
    assert!(matches!(
        cli(s, &["migrate", "upstream", "-c", "x"]),
        Cli::Migrate(MigrateArgs::Upstream { .. })
    ));
    assert!(matches!(
        cli(s, &["migrate", "builtin"]),
        Cli::Migrate(MigrateArgs::Builtin { .. })
    ));
    let Cli::Doctor(d) = cli(s, &["doctor", "--fix", "--stash"]) else {
        panic!()
    };
    assert!(d.fix && d.stash && !d.fast_path);
    assert!(matches!(
        cli(s, &["self", "alias", "install", "--dir", "/b", "-f"]),
        Cli::SelfCmd(SelfArgs::AliasInstall { force: true, .. })
    ));
    assert!(matches!(
        cli(s, &["self", "alias", "uninstall"]),
        Cli::SelfCmd(SelfArgs::AliasUninstall)
    ));
    assert!(matches!(
        cli(s, &["self", "alias", "status"]),
        Cli::SelfCmd(SelfArgs::AliasStatus)
    ));
    assert!(matches!(
        cli(s, &["self", "check"]),
        Cli::SelfCmd(SelfArgs::Check)
    ));
    for (name, shell) in [
        ("bash", Shell::Bash),
        ("zsh", Shell::Zsh),
        ("fish", Shell::Fish),
        ("powershell", Shell::PowerShell),
    ] {
        let Cli::Completions(got) = cli(s, &["completions", name]) else {
            panic!()
        };
        assert_eq!(format!("{got:?}"), format!("{shell:?}"));
    }
    assert_eq!(
        last_line(&usage(s, &["cache"])),
        "snp cache: error: the following arguments are required: cache_cmd"
    );
}

#[test]
fn version_by_brand() {
    let CliOutcome::Stdout(v) = run(Brand::PreCommit, &["-V"]) else {
        panic!()
    };
    assert_eq!(v, "pre-commit 4.7.0\n");
    let CliOutcome::Stdout(v) = run(Brand::Snp, &["--version"]) else {
        panic!()
    };
    assert_eq!(v, format!("snp {}\n", snp_core::profile::SNP_VERSION));
    assert_eq!(v, "snp 2.0.1\n");
    assert_eq!(snp_core::profile::PRE_COMMIT, "4.7.0");
}

#[test]
fn help_reparses() {
    for brand in BOTH {
        let top = run(brand, &["-h"]);
        assert!(matches!(&top, CliOutcome::Stdout(s) if s.starts_with("usage: ")));
        for a in [
            &["help"][..],
            &["help", ""][..],
            &["help", "--", "--bogus"][..],
        ] {
            assert_eq!(format!("{:?}", run(brand, a)), format!("{top:?}"));
        }
        assert_eq!(
            format!("{:?}", run(brand, &["help", "run"])),
            format!("{:?}", run(brand, &["run", "--help"]))
        );
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_argv_round_trips() {
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    let raw = |b: &[u8]| OsString::from_vec(b.to_vec());
    let env = Env::from_pairs::<&str, &str>([]);
    for brand in BOTH {
        let a = vec![
            OsString::from("run"),
            raw(b"h\xff"),
            OsString::from("--files"),
            raw(b"\xfe.py"),
            raw(b"caf\xc3\xa9"),
            OsString::from("--remote-url"),
            raw(b"u\x80"),
        ];
        let CliOutcome::Cli(Cli::Run(r)) = parse(brand, &a, &env, None) else {
            panic!()
        };
        assert_eq!(r.hooks[0].as_bytes(), b"h\xff");
        assert_eq!(r.files[0].as_bytes(), b"\xfe.py");
        assert_eq!(r.files[1].as_bytes(), "café".as_bytes());
        assert_eq!(r.remote_url.unwrap().as_bytes(), b"u\x80");
        let a = vec![
            OsString::from("hook-impl"),
            OsString::from("--hook-type=pre-push"),
            OsString::from("--"),
            raw(b"\xff"),
        ];
        let CliOutcome::Cli(Cli::HookImpl(h)) = parse(brand, &a, &env, None) else {
            panic!()
        };
        assert_eq!(h.rest[1].as_bytes(), b"\xff");
        let a = vec![
            OsString::from("run"),
            OsString::from("--hook-stage"),
            raw(b"\xff"),
        ];
        let CliOutcome::Usage(u) = parse(brand, &a, &env, None) else {
            panic!()
        };
        assert!(u.ends_with("invalid choice: '\\udcff' (choose from 'commit-msg', 'post-checkout', 'post-commit', 'post-merge', 'post-rewrite', 'pre-commit', 'pre-merge-commit', 'pre-push', 'pre-rebase', 'prepare-commit-msg', 'manual')\n"), "{u}");
    }
}

#[test]
fn columns_come_from_the_environment() {
    let narrow = run_env(Brand::PreCommit, &["clean", "-h"], &[("COLUMNS", "40")]);
    let wide = run_env(Brand::PreCommit, &["clean", "-h"], &[("COLUMNS", "200")]);
    assert_ne!(format!("{narrow:?}"), format!("{wide:?}"));
    let tty = parse(
        Brand::PreCommit,
        &argv(&["clean", "-h"]),
        &Env::from_pairs::<&str, &str>([]),
        Some(40),
    );
    assert_eq!(format!("{tty:?}"), format!("{narrow:?}"));
}

/// `COLUMNS` goes through Python `int()`, which does not strip U+001C..U+001F: CPython falls back to 80 columns.
#[test]
fn columns_with_an_ascii_separator_fall_back_to_80() {
    for brand in BOTH {
        let odd = run_env(brand, &["clean", "-h"], &[("COLUMNS", "\x1c40")]);
        let default = run_env(brand, &["clean", "-h"], &[]);
        let narrow = run_env(brand, &["clean", "-h"], &[("COLUMNS", "40")]);
        assert_eq!(format!("{odd:?}"), format!("{default:?}"), "{brand:?}");
        assert_ne!(format!("{odd:?}"), format!("{narrow:?}"), "{brand:?}");
    }
}

/// `-j/--jobs` is `type=int`; `int('\x1c5')` raises, so argparse reports an invalid int value.
#[test]
fn jobs_with_an_ascii_separator_is_an_invalid_int() {
    for brand in BOTH {
        for v in [
            "\x1c5", "5\x1c", "\x1d5", "5\x1d", "\x1e5", "5\x1e", "\x1f5", "5\x1f",
        ] {
            let err = usage(brand, &["autoupdate", "-j", v]);
            assert_eq!(
                last_line(&err),
                format!(
                    "{} autoupdate: error: argument -j/--jobs: invalid int value: {}",
                    brand.prog(),
                    py_repr(v)
                ),
                "{brand:?} {v:?}"
            );
        }
    }
}

/// `repr()` of the four separators as CPython prints them.
fn py_repr(v: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("'");
    for c in v.chars() {
        if c.is_ascii_control() {
            write!(out, "\\x{:02x}", u32::from(c)).unwrap();
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[test]
fn completions_cover_every_option() {
    fn walk(spec: &'static snp_argparse::ParserSpec, out: &mut Vec<&'static str>) {
        for a in spec.actions {
            if let snp_argparse::Kind::SubParsers(subs) = a.kind {
                for s in subs {
                    out.push(s.name);
                    walk(s.spec, out);
                }
            }
            out.extend(a.opts.iter().filter(|o| o.starts_with("--")));
        }
    }
    let mut names = Vec::new();
    walk(specs::SNP_SPEC, &mut names);
    assert_eq!(crate::COMPLETIONS.len(), 4);
    for (shell, script) in crate::COMPLETIONS {
        for n in &names {
            let bare = n.trim_start_matches('-');
            assert!(script.contains(bare), "{shell:?} completion lacks {n}");
        }
    }
}

#[test]
fn config_given_versus_default() {
    for brand in BOTH {
        // the default's own value, given explicitly, is still explicit (an installed script's `--config=…` wins)
        let Cli::Run(r) = cli(brand, &["run", "-c", ".pre-commit-config.yaml"]) else {
            panic!()
        };
        assert_eq!(r.config, ConfigArg::given(".pre-commit-config.yaml"));
        assert_eq!(
            r.config.explicit(),
            Some(OsStr::new(".pre-commit-config.yaml"))
        );
        let Cli::Install(i) = cli(brand, &["install"]) else {
            panic!()
        };
        assert!(!i.config.given);
        let Cli::MigrateConfig(m) = cli(brand, &["migrate-config", "--config=.snp.yaml"]) else {
            panic!()
        };
        assert_eq!(m.config, ConfigArg::given(".snp.yaml"));
    }
}
