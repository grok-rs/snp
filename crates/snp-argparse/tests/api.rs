//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.3 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-12 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};

    use snp_argparse::{
        ActionSpec, Conv, DefaultValue, Kind, Namespace, Nargs, Outcome, ParseCfg, ParserSpec,
        SubCommand, Value, textwrap,
    };
    use snp_core::env::Env;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    fn conv(s: &OsStr) -> Result<OsString, String> {
        s.to_str().map(OsString::from).ok_or_else(String::new)
    }

    static EMPTY: ParserSpec = ParserSpec {
        prog_suffix: "",
        description: None,
        epilog: None,
        actions: &[],
        allow_abbrev: true,
        add_help: true,
    };
    static SUBS: [SubCommand; 1] = [SubCommand {
        name: "run",
        help: Some("Run hooks."),
        listed: true,
        spec: &EMPTY,
    }];
    static ACTIONS: [ActionSpec; 1] = [ActionSpec {
        opts: &["-c", "--config"],
        dest: "config",
        nargs: Nargs::One,
        kind: Kind::Store,
        conv: Conv::Identity,
        choices: Some(&["a", "b"]),
        default: DefaultValue::Str(".pre-commit-config.yaml"),
        required: false,
        help: Some("Path to alternate config file"),
        metavar: Some("CONFIG"),
        mutex_group: Some(0),
        hidden: false,
    }];
    static TOP: ParserSpec = ParserSpec {
        prog_suffix: "",
        description: Some("d"),
        epilog: Some("e"),
        actions: &ACTIONS,
        allow_abbrev: false,
        add_help: false,
    };

    #[test]
    fn send_sync_types() {
        send_sync::<Nargs>();
        send_sync::<Kind>();
        send_sync::<SubCommand>();
        send_sync::<Conv>();
        send_sync::<DefaultValue>();
        send_sync::<ActionSpec>();
        send_sync::<ParserSpec>();
        send_sync::<ParseCfg<'_>>();
        send_sync::<Outcome>();
        send_sync::<Value>();
        send_sync::<Namespace>();
    }

    #[test]
    fn spec_types() {
        has::<[Nargs; 7]>([
            Nargs::Zero,
            Nargs::One,
            Nargs::Optional,
            Nargs::ZeroOrMore,
            Nargs::OneOrMore,
            Nargs::Remainder,
            Nargs::Parser,
        ]);
        has::<[Kind; 8]>([
            Kind::Store,
            Kind::StoreTrue,
            Kind::StoreFalse,
            Kind::StoreConst("c"),
            Kind::Append,
            Kind::Help,
            Kind::Version,
            Kind::SubParsers(&SUBS),
        ]);
        has::<[Conv; 6]>([
            Conv::Identity,
            Conv::UseColor,
            Conv::Stage,
            Conv::HookType,
            Conv::Int,
            Conv::Custom(conv, "conv"),
        ]);
        has::<[DefaultValue; 8]>([
            DefaultValue::None,
            DefaultValue::Str("s"),
            DefaultValue::Env {
                var: "PRE_COMMIT_COLOR",
                fallback: "auto",
            },
            DefaultValue::False,
            DefaultValue::True,
            DefaultValue::Int(1),
            DefaultValue::EmptyList,
            DefaultValue::Const("c"),
        ]);
        let ParserSpec {
            prog_suffix,
            description,
            epilog,
            actions,
            allow_abbrev,
            add_help,
        }: &'static ParserSpec = &TOP;
        has::<(&str, Option<&str>, Option<&str>, bool, bool)>((
            *prog_suffix,
            *description,
            *epilog,
            *allow_abbrev,
            *add_help,
        ));
        has::<&'static [ActionSpec]>(actions);
        let SubCommand {
            name,
            help,
            listed,
            spec,
        }: SubCommand = SUBS[0];
        has::<(&str, Option<&str>, bool, &'static ParserSpec)>((name, help, listed, spec));
        has::<[Value; 5]>([
            Value::Unset,
            Value::Bool(true),
            Value::Str(OsString::new()),
            Value::List(Vec::new()),
            Value::Int(1),
        ]);
        has::<Outcome>(Outcome::Help(String::new()));
        has::<Outcome>(Outcome::Version(String::new()));
        has::<Outcome>(Outcome::Usage {
            stderr: String::new(),
        });
        has::<fn(Namespace) -> Outcome>(Outcome::Parsed);
        let env = Env::from_pairs::<&str, &str>([]);
        let cfg = ParseCfg {
            prog: "pre-commit",
            env: &env,
            columns: 80,
        };
        has::<String>(format!("{cfg:?}{TOP:?}"));
    }

    #[test]
    fn functions() {
        has::<for<'a> fn(&'a Namespace) -> Option<(&'static str, &'a Namespace)>>(
            Namespace::command,
        );
        has::<fn(&Namespace, &str) -> bool>(Namespace::flag);
        has::<for<'a> fn(&'a Namespace, &str) -> Option<&'a OsStr>>(Namespace::value);
        has::<for<'a> fn(&'a Namespace, &str) -> &'a [OsString]>(Namespace::list);
        has::<fn(&Namespace, &str) -> Option<i64>>(Namespace::int);
        has::<fn(&Namespace, &str) -> bool>(Namespace::was_given);
        has::<fn(&Namespace) -> Namespace>(Namespace::clone);
        has::<fn(&'static ParserSpec, &[OsString], &ParseCfg<'_>) -> Outcome>(snp_argparse::parse);
        has::<fn(&'static ParserSpec, &[&str], &ParseCfg<'_>) -> String>(snp_argparse::format_help);
        has::<fn(&'static ParserSpec, &[&str], &ParseCfg<'_>) -> String>(
            snp_argparse::format_usage,
        );
        has::<fn(&Env, Option<usize>) -> usize>(snp_argparse::columns);
        has::<fn(&'static ParserSpec, &'static ParserSpec) -> Result<(), String>>(
            snp_argparse::prefix_stable,
        );
        has::<fn(&'static ParserSpec, &str) -> String>(snp_argparse::export_json);
        has::<fn(&str, usize) -> Vec<String>>(textwrap::wrap);
        has::<fn(&str, usize, &str, &str) -> String>(textwrap::fill);
    }
}
