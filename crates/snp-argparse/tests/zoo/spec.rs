// The "zoo" specs: every argparse feature snp-argparse models, beyond what pre-commit's own parser uses
// (`nargs='+'`, option REMAINDER, `store_const` sharing a dest, required options and sub-parsers, unlisted and
// help-less commands, `allow_abbrev=False`, `add_help=False`, negative-number-like options, hidden actions,
// custom converters, descriptions and epilogs). `cargo xtask cli-goldens` exports them with
// `snp_argparse::export_json` and records what a real CPython 3.13 argparse does with them
// (`crates/snp-argparse/tests/goldens/zoo.json`); `tests/zoo.rs` replays the goldens.
//
// This file is included by `tests/zoo.rs` and by `xtask/src/tasks/cli_goldens.rs`: plain comments only.
//
// Owner: WP-12.

use snp_argparse::{ActionSpec, Conv, DefaultValue, Kind, Nargs, ParserSpec, SubCommand};

const fn store(opts: &'static [&'static str], dest: &'static str) -> ActionSpec {
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

const fn flag(opts: &'static [&'static str], dest: &'static str, kind: Kind) -> ActionSpec {
    ActionSpec {
        nargs: Nargs::Zero,
        kind,
        default: match kind {
            Kind::StoreFalse => DefaultValue::True,
            Kind::StoreTrue => DefaultValue::False,
            _ => DefaultValue::None,
        },
        ..store(opts, dest)
    }
}

const fn parser(prog_suffix: &'static str, actions: &'static [ActionSpec]) -> ParserSpec {
    ParserSpec {
        prog_suffix,
        description: None,
        epilog: None,
        actions,
        allow_abbrev: true,
        add_help: true,
    }
}

/// `type=upper`: `ValueError` unless the value is upper case (`str.isupper()`).
fn upper(s: &std::ffi::OsStr) -> Result<std::ffi::OsString, String> {
    let t = s.to_str().ok_or_else(String::new)?;
    let cased = t.chars().any(char::is_alphabetic);
    if cased && t.chars().all(|c| !c.is_lowercase()) {
        Ok(s.to_os_string())
    } else {
        Err(String::new())
    }
}

/// `type=nonempty`: `ArgumentTypeError('must not be empty')` for `''`.
fn nonempty(s: &std::ffi::OsStr) -> Result<std::ffi::OsString, String> {
    if s.is_empty() {
        Err("must not be empty".to_owned())
    } else {
        Ok(s.to_os_string())
    }
}

/// `zoo`: options of every kind, a mutually exclusive group sharing a dest, two positionals.
pub(crate) static ZOO_MAIN: ParserSpec = ParserSpec {
    description: Some(
        "%(prog)s exercises\tevery   action kind.  It is long enough to wrap at the narrow widths of the \
         golden matrix, with 100%% of the words.",
    ),
    epilog: Some("See also: %(prog)s --help.  Report bugs to nobody."),
    ..parser(
        "",
        &[
            ActionSpec {
                conv: Conv::Int,
                default: DefaultValue::Int(3),
                help: Some("how many (default %(default)s, type %(type)s)"),
                ..store(&["-n", "--count"], "count")
            },
            ActionSpec {
                conv: Conv::Custom(upper, "upper"),
                default: DefaultValue::Str("X"),
                metavar: Some("NAME"),
                help: Some("an upper-case name (default %(default)r)"),
                ..store(&["--name"], "name")
            },
            ActionSpec {
                conv: Conv::Custom(nonempty, "nonempty"),
                help: Some("anything but the empty string"),
                ..store(&["--label"], "label")
            },
            ActionSpec {
                choices: Some(&["low", "mid", "high"]),
                default: DefaultValue::Str("mid"),
                help: Some("one of %(choices)s; %(default)s by default"),
                ..store(&["--level"], "level")
            },
            flag(&["-q"], "quiet", Kind::StoreTrue),
            ActionSpec {
                help: Some("turn the volume off"),
                ..flag(&["-Q", "--no-volume"], "volume", Kind::StoreFalse)
            },
            ActionSpec {
                kind: Kind::Append,
                metavar: Some("TAG"),
                help: Some("repeatable"),
                ..store(&["-t", "--tag"], "tags")
            },
            ActionSpec {
                nargs: Nargs::Optional,
                kind: Kind::StoreConst("C"),
                help: Some("optional value, %(const)s when bare"),
                ..store(&["--opt"], "opt")
            },
            ActionSpec {
                nargs: Nargs::OneOrMore,
                help: Some("one or more values"),
                ..store(&["--many"], "many")
            },
            ActionSpec {
                nargs: Nargs::ZeroOrMore,
                choices: Some(&["a", "b"]),
                help: Some("zero or more of %(choices)s"),
                ..store(&["--some"], "some")
            },
            ActionSpec {
                kind: Kind::StoreConst("fast"),
                nargs: Nargs::Zero,
                default: DefaultValue::Str("normal"),
                mutex_group: Some(0),
                help: Some("mode %(const)s"),
                ..store(&["--fast"], "mode")
            },
            ActionSpec {
                kind: Kind::StoreConst("slow"),
                nargs: Nargs::Zero,
                mutex_group: Some(0),
                help: Some("mode %(const)s"),
                ..store(&["--slow"], "mode")
            },
            ActionSpec {
                hidden: true,
                mutex_group: Some(0),
                ..flag(&["--secret"], "secret", Kind::StoreTrue)
            },
            ActionSpec {
                required: true,
                help: Some("a required option"),
                ..store(&["-r", "--req"], "req")
            },
            ActionSpec {
                hidden: true,
                ..store(&["--hidden"], "hidden")
            },
            ActionSpec {
                help: Some("the source"),
                ..store(&[], "src")
            },
            ActionSpec {
                nargs: Nargs::ZeroOrMore,
                metavar: Some("FILE"),
                help: Some("files"),
                ..store(&[], "files")
            },
            ActionSpec {
                nargs: Nargs::Remainder,
                help: Some("everything after"),
                ..store(&["--rem"], "rem")
            },
        ],
    )
};

static SUB_LISTED: ParserSpec = ParserSpec {
    description: Some("The listed command."),
    ..parser(
        " listed",
        &[
            ActionSpec {
                nargs: Nargs::OneOrMore,
                help: Some("one or more items"),
                ..store(&[], "items")
            },
            flag(&["-1"], "one", Kind::StoreTrue),
            ActionSpec {
                conv: Conv::Int,
                ..store(&["--num"], "num")
            },
        ],
    )
};

static SUB_STRICT: ParserSpec = ParserSpec {
    allow_abbrev: false,
    add_help: false,
    ..parser(
        " strict",
        &[
            flag(&["--verbose"], "verbose", Kind::StoreTrue),
            flag(&["--version-info"], "version_info", Kind::StoreTrue),
            ActionSpec {
                nargs: Nargs::Optional,
                ..store(&[], "maybe")
            },
        ],
    )
};

static SUB_HIDDEN: ParserSpec = parser(" hidden-cmd", &[store(&[], "x")]);
static SUB_QUIET: ParserSpec = parser(" quiet", &[]);

static NESTED_LEAF: ParserSpec = parser(" leaf", &[flag(&["--deep"], "deep", Kind::StoreTrue)]);
static NESTED_COMMANDS: [SubCommand; 1] = [SubCommand {
    name: "leaf",
    help: Some("the leaf"),
    listed: true,
    spec: &NESTED_LEAF,
}];
static SUB_NESTED: ParserSpec = parser(
    " nested",
    &[ActionSpec {
        nargs: Nargs::Parser,
        kind: Kind::SubParsers(&NESTED_COMMANDS),
        required: true,
        ..store(&[], "leaf_cmd")
    }],
);

static ZOO_COMMANDS: [SubCommand; 5] = [
    SubCommand {
        name: "listed",
        help: Some("a listed command with a %(prog)s-free help"),
        listed: true,
        spec: &SUB_LISTED,
    },
    SubCommand {
        name: "strict",
        help: Some("no abbreviations, no -h"),
        listed: true,
        spec: &SUB_STRICT,
    },
    SubCommand {
        name: "hidden-cmd",
        help: Some("never shown"),
        listed: false,
        spec: &SUB_HIDDEN,
    },
    SubCommand {
        name: "quiet",
        help: None,
        listed: true,
        spec: &SUB_QUIET,
    },
    SubCommand {
        name: "nested",
        help: Some("has its own sub-commands"),
        listed: true,
        spec: &SUB_NESTED,
    },
];

/// `zoo-sub`: required sub-parsers, unlisted and help-less commands, nesting, `allow_abbrev=False`,
/// `add_help=False`, a negative-number-like option.
pub(crate) static ZOO_SUB: ParserSpec = parser(
    "",
    &[
        ActionSpec {
            nargs: Nargs::Zero,
            kind: Kind::Version,
            default: DefaultValue::Const(
                "%(prog)s version 1.2.3 (a long version string that wraps)",
            ),
            help: Some("show the version"),
            ..store(&["--version"], "version")
        },
        flag(&["--top"], "top", Kind::StoreTrue),
        ActionSpec {
            nargs: Nargs::Parser,
            kind: Kind::SubParsers(&ZOO_COMMANDS),
            required: true,
            help: Some("the command to run"),
            ..store(&[], "cmd")
        },
    ],
);

/// `zoo-files`: the shape of a pre-commit-hooks console script (`nargs='+'` file names, an int option).
pub(crate) static ZOO_FILES: ParserSpec = ParserSpec {
    description: Some("Checks files for things."),
    ..parser(
        "",
        &[
            ActionSpec {
                nargs: Nargs::OneOrMore,
                help: Some("Filenames to check"),
                ..store(&[], "filenames")
            },
            ActionSpec {
                conv: Conv::Int,
                default: DefaultValue::Int(500),
                help: Some("Maximum allowable KB for added files"),
                ..store(&["--maxkb"], "maxkb")
            },
            flag(&["--enforce-all"], "enforce_all", Kind::StoreTrue),
            ActionSpec {
                default: DefaultValue::Env {
                    var: "ZOO_MODE",
                    fallback: "lf",
                },
                choices: Some(&["auto", "crlf", "lf", "no"]),
                help: Some("Replace line ending with the specified. Default is \"%(default)s\""),
                ..store(&["--fix", "-f"], "fix")
            },
        ],
    )
};

/// Every zoo spec by name (the `spec` field of the goldens).
pub(crate) static ZOO: [(&str, &ParserSpec); 3] = [
    ("main", &ZOO_MAIN),
    ("sub", &ZOO_SUB),
    ("files", &ZOO_FILES),
];
