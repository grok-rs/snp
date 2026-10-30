//! `const` builders for the console scripts' `ParserSpec` tables (`parser.add_argument(…)` as data).
//!
//! Every pre-commit-hooks console script builds `argparse.ArgumentParser()` (prog = the script's basename,
//! `allow_abbrev=True`, `add_help=True`) and adds its arguments in source order; the tables in `pch::*` list them
//! in that order, which drives usage, help sections and abbreviation listings exactly as argparse does.
//!
//! Owner: WP-26 (milestone M1).

use snp_argparse::{ActionSpec, Conv, DefaultValue, Kind, Nargs};

/// `add_argument(*opts)`: a `store` option taking one value (`default=None`, no help).
pub const fn store(opts: &'static [&'static str], dest: &'static str) -> ActionSpec {
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

/// `add_argument(*opts, action='store_true')`.
pub const fn store_true(opts: &'static [&'static str], dest: &'static str) -> ActionSpec {
    ActionSpec {
        nargs: Nargs::Zero,
        kind: Kind::StoreTrue,
        default: DefaultValue::False,
        ..store(opts, dest)
    }
}

/// `add_argument(*opts, action='append')` (`default=None` unless [`with_default`] says otherwise).
pub const fn append(opts: &'static [&'static str], dest: &'static str) -> ActionSpec {
    ActionSpec {
        kind: Kind::Append,
        ..store(opts, dest)
    }
}

/// `add_argument(*opts, type=int, default=default)`.
pub const fn int_opt(
    opts: &'static [&'static str],
    dest: &'static str,
    default: i64,
) -> ActionSpec {
    ActionSpec {
        conv: Conv::Int,
        default: DefaultValue::Int(default),
        ..store(opts, dest)
    }
}

/// `add_argument(dest, nargs=nargs)`: a positional.
pub const fn positional(dest: &'static str, nargs: Nargs) -> ActionSpec {
    ActionSpec {
        nargs,
        ..store(&[], dest)
    }
}

/// `add_argument('filenames', nargs='*', help=help)`.
pub const fn files_positional(help: Option<&'static str>) -> ActionSpec {
    with_help(positional("filenames", Nargs::ZeroOrMore), help)
}

/// `help=` (`None` for no help string).
#[expect(
    clippy::needless_pass_by_value,
    reason = "const builder: the struct update moves the spec"
)]
pub const fn with_help(a: ActionSpec, help: Option<&'static str>) -> ActionSpec {
    ActionSpec { help, ..a }
}

/// `metavar=`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "const builder: the struct update moves the spec"
)]
pub const fn with_metavar(a: ActionSpec, metavar: &'static str) -> ActionSpec {
    ActionSpec {
        metavar: Some(metavar),
        ..a
    }
}

/// `choices=`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "const builder: the struct update moves the spec"
)]
pub const fn with_choices(a: ActionSpec, choices: &'static [&'static str]) -> ActionSpec {
    ActionSpec {
        choices: Some(choices),
        ..a
    }
}

/// `default=`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "const builder: the struct update moves the spec"
)]
pub const fn with_default(a: ActionSpec, default: DefaultValue) -> ActionSpec {
    ActionSpec { default, ..a }
}
