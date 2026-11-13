//! `cargo xtask completions [--check]`: shell completion scripts and the man page generated from
//! `snp_cli::SNP_SPEC` (ARCHITECTURE §9.1, ADR-010).
//!
//! A `clap::Command` is built from the same `ParserSpec` tables the runtime parser uses (clap never reaches the
//! binary) and rendered with `clap_complete` (bash, zsh, fish, PowerShell) and `clap_mangen` (`snp.1`) into
//! `crates/snp-cli/completions/`, which `snp_cli::COMPLETIONS` embeds with `include_str!`. `--check` fails when the
//! committed files are stale. The unit tests assert that the clap command carries exactly the option strings,
//! positionals and sub-commands of `SNP_SPEC`.
//!
//! Owner: WP-12.

use std::ffi::OsString;

use anyhow::{bail, ensure};
use clap::{Arg, ArgAction, Command};
use clap_complete::Shell as ClapShell;
use snp_argparse::{ActionSpec, Kind, Nargs, ParserSpec};

use crate::workspace;

const OUT_DIR: &str = "crates/snp-cli/completions";

/// Every file this task writes: (file name, generator).
const FILES: &[(&str, Option<ClapShell>)] = &[
    ("snp.bash", Some(ClapShell::Bash)),
    ("_snp", Some(ClapShell::Zsh)),
    ("snp.fish", Some(ClapShell::Fish)),
    ("_snp.ps1", Some(ClapShell::PowerShell)),
    ("snp.1", None),
];

/// The help text for completion descriptions: `%(default)s`, `%(choices)s` and `%%` expanded.
fn describe(a: &ActionSpec) -> Option<String> {
    let help = a.help?;
    let default = match a.default {
        snp_argparse::DefaultValue::Str(s) | snp_argparse::DefaultValue::Const(s) => s.to_owned(),
        snp_argparse::DefaultValue::Env { fallback, .. } => fallback.to_owned(),
        snp_argparse::DefaultValue::Int(n) => n.to_string(),
        _ => String::new(),
    };
    let choices = a.choices.map_or_default(|c| c.join(", "));
    Some(
        help.replace("%(default)s", &default)
            .replace("%(choices)s", &choices)
            .replace("%(prog)s", "snp")
            .replace("%%", "%")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// One argparse action as a clap argument.
fn arg(a: &'static ActionSpec, id: &'static str) -> Arg {
    let mut arg = Arg::new(id);
    if a.opts.is_empty() {
        let name = a.metavar.unwrap_or(a.dest);
        arg = arg.value_name(name);
    } else {
        let mut longs = a.opts.iter().filter_map(|o| o.strip_prefix("--"));
        let mut shorts = a
            .opts
            .iter()
            .filter(|o| !o.starts_with("--"))
            .filter_map(|o| o.strip_prefix('-').and_then(|s| s.chars().next()));
        if let Some(l) = longs.next() {
            arg = arg.long(l);
        }
        for l in longs {
            arg = arg.visible_alias(l);
        }
        if let Some(s) = shorts.next() {
            arg = arg.short(s);
        }
        for s in shorts {
            arg = arg.visible_short_alias(s);
        }
        if let Some(m) = a.metavar {
            arg = arg.value_name(m);
        }
    }
    if let Some(h) = describe(a) {
        arg = arg.help(h);
    }
    if a.hidden {
        arg = arg.hide(true);
    }
    if let Some(c) = a.choices {
        arg = arg.value_parser(clap::builder::PossibleValuesParser::new(c.iter().copied()));
    }
    let flag = matches!(
        a.kind,
        Kind::StoreTrue | Kind::StoreFalse | Kind::Help | Kind::Version
    ) || (matches!(a.kind, Kind::StoreConst(_)) && !matches!(a.nargs, Nargs::Optional));
    arg = match a.kind {
        Kind::Help => arg.action(ArgAction::Help),
        Kind::Version => arg.action(ArgAction::Version),
        _ if flag => arg.action(ArgAction::SetTrue),
        Kind::Append => arg.action(ArgAction::Append),
        _ => arg.action(ArgAction::Set),
    };
    if !flag {
        arg = match a.nargs {
            Nargs::Optional => arg.num_args(0..=1),
            Nargs::ZeroOrMore => arg.num_args(0..),
            Nargs::OneOrMore => arg.num_args(1..),
            Nargs::Remainder => arg
                .num_args(0..)
                .trailing_var_arg(true)
                .allow_hyphen_values(true),
            _ => arg.num_args(1),
        };
    }
    if a.opts.is_empty() && matches!(a.nargs, Nargs::One | Nargs::OneOrMore) {
        arg = arg.required(true);
    }
    arg
}

/// The clap command of one parser (sub-commands included).
fn command(spec: &'static ParserSpec, name: &'static str) -> Command {
    let mut cmd = Command::new(name)
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true);
    if spec.add_help {
        cmd = cmd.arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help("show this help message and exit"),
        );
    }
    let mut ids: Vec<&'static str> = vec!["help"];
    for a in spec.actions {
        if let Kind::SubParsers(subs) = a.kind {
            for s in subs {
                let mut sc = command(s.spec, s.name);
                sc = match s.help {
                    Some(h) if s.listed => sc.about(h),
                    _ => sc.hide(true),
                };
                cmd = cmd.subcommand(sc);
            }
            continue;
        }
        // clap ids must be unique: the dest, else the first unused option string.
        let Some(id) = std::iter::once(a.dest)
            .chain(a.opts.iter().copied())
            .find(|c| !c.is_empty() && !ids.contains(c))
        else {
            continue;
        };
        ids.push(id);
        cmd = cmd.arg(arg(a, id));
    }
    cmd
}

/// The top-level `snp` command.
fn snp_command() -> Command {
    command(snp_cli::SNP_SPEC, "snp")
        .version(snp_core::profile::SNP_VERSION)
        .about("You Shell Not Pass: a fast, drop-in pre-commit runner")
}

fn render(file: &str, shell: Option<ClapShell>) -> anyhow::Result<Vec<u8>> {
    let mut cmd = snp_command();
    let mut out = Vec::new();
    match shell {
        Some(sh) => clap_complete::generate(sh, &mut cmd, "snp", &mut out),
        None => clap_mangen::Man::new(cmd).render(&mut out)?,
    }
    ensure!(!out.is_empty(), "{file}: empty output");
    Ok(out)
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => bail!("usage: cargo xtask completions [--check]"),
    };
    let root = workspace::root()?;
    let dir = root.join(OUT_DIR);
    std::fs::create_dir_all(&dir)?;
    let mut stale = Vec::new();
    for &(file, shell) in FILES {
        let text = render(file, shell)?;
        let path = dir.join(file);
        if check {
            if std::fs::read(&path).ok().as_deref() != Some(text.as_slice()) {
                stale.push(workspace::rel(&root, &path));
            }
        } else {
            std::fs::write(&path, &text)?;
            workspace::say(&format!(
                "completions: wrote {}",
                workspace::rel(&root, &path)
            ));
        }
    }
    ensure!(
        stale.is_empty(),
        "stale completions (run `cargo xtask completions`): {}",
        stale.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// (command path, item) for every option string, positional and sub-command of a spec.
    fn spec_items(spec: &'static ParserSpec, path: &str, out: &mut BTreeSet<(String, String)>) {
        if spec.add_help {
            out.insert((path.to_owned(), "-h".to_owned()));
            out.insert((path.to_owned(), "--help".to_owned()));
        }
        for a in spec.actions {
            if let Kind::SubParsers(subs) = a.kind {
                for s in subs {
                    out.insert((path.to_owned(), format!("cmd:{}", s.name)));
                    spec_items(s.spec, &format!("{path} {}", s.name), out);
                }
            } else if a.opts.is_empty() {
                out.insert((
                    path.to_owned(),
                    format!("pos:{}", a.metavar.unwrap_or(a.dest)),
                ));
            } else {
                for o in a.opts {
                    out.insert((path.to_owned(), (*o).to_owned()));
                }
            }
        }
    }

    fn clap_items(cmd: &Command, path: &str, out: &mut BTreeSet<(String, String)>) {
        for a in cmd.get_arguments() {
            if a.is_positional() {
                let name = a
                    .get_value_names()
                    .and_then(|v| v.first())
                    .map_or_default(ToString::to_string);
                out.insert((path.to_owned(), format!("pos:{name}")));
                continue;
            }
            for l in a.get_long_and_visible_aliases().unwrap_or_default() {
                out.insert((path.to_owned(), format!("--{l}")));
            }
            for s in a.get_short_and_visible_aliases().unwrap_or_default() {
                out.insert((path.to_owned(), format!("-{s}")));
            }
        }
        for s in cmd.get_subcommands() {
            out.insert((path.to_owned(), format!("cmd:{}", s.get_name())));
            clap_items(s, &format!("{path} {}", s.get_name()), out);
        }
    }

    #[test]
    fn clap_command_has_exactly_the_options_of_snp_spec() {
        let cmd = snp_command();
        cmd.clone().debug_assert();
        let mut want = BTreeSet::new();
        spec_items(snp_cli::SNP_SPEC, "snp", &mut want);
        let mut got = BTreeSet::new();
        clap_items(&cmd, "snp", &mut got);
        assert_eq!(got, want);
        assert!(want.contains(&("snp run".to_owned(), "--stdin-files".to_owned())));
        assert!(want.contains(&("snp".to_owned(), "cmd:completions".to_owned())));
    }

    /// The committed scripts (shipped through `snp_cli::COMPLETIONS`) and the man page are exactly what the
    /// generators render from `SNP_SPEC` now: `cargo xtask completions --check` as a test, so editing the spec
    /// without regenerating, or hand-editing a script, fails wherever the workspace tests run.
    #[test]
    fn committed_completions_are_fresh() {
        let dir = workspace::root().unwrap().join(OUT_DIR);
        for &(file, shell) in FILES {
            let want = render(file, shell).unwrap();
            let got = std::fs::read(dir.join(file)).unwrap();
            assert!(
                got == want,
                "{OUT_DIR}/{file} is stale: run `cargo xtask completions`"
            );
        }
    }

    #[test]
    fn every_generator_renders() {
        for &(file, shell) in FILES {
            let text = String::from_utf8(render(file, shell).unwrap()).unwrap();
            let needle = if shell.is_some() {
                "stdin-files"
            } else {
                "snp\\-run(1)"
            };
            assert!(text.contains(needle), "{file}");
        }
    }
}
