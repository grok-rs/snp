//! [`crate::export_json`]: the spec tree as JSON for `xtask/oracle/argparse_from_json.py` (ADR-058).
//!
//! Owner: WP-12.

use std::fmt::Write as _;

use crate::pystr::write_json_str;
use crate::view::{self, MAX_DEPTH};
use crate::{ActionSpec, DefaultValue, Kind, Nargs, ParserSpec};

/// `{"prog": prog, "parser": …}`.
pub(crate) fn export(spec: &'static ParserSpec, prog: &str) -> String {
    let mut out = String::from("{\"prog\": ");
    write_json_str(&mut out, prog);
    out.push_str(", \"parser\": ");
    parser(&mut out, spec, 0);
    out.push_str("}\n");
    out
}

fn opt_str(out: &mut String, s: Option<&str>) {
    match s {
        Some(s) => write_json_str(out, s),
        None => out.push_str("null"),
    }
}

fn bool_str(out: &mut String, b: bool) {
    out.push_str(if b { "true" } else { "false" });
}

fn parser(out: &mut String, spec: &'static ParserSpec, depth: usize) {
    out.push_str("{\"prog_suffix\": ");
    write_json_str(out, spec.prog_suffix);
    out.push_str(", \"description\": ");
    opt_str(out, spec.description);
    out.push_str(", \"epilog\": ");
    opt_str(out, spec.epilog);
    out.push_str(", \"allow_abbrev\": ");
    bool_str(out, spec.allow_abbrev);
    out.push_str(", \"add_help\": ");
    bool_str(out, spec.add_help);
    out.push_str(", \"actions\": [");
    for (i, a) in spec.actions.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        action(out, a, depth);
    }
    out.push_str("]}");
}

fn action(out: &mut String, a: &'static ActionSpec, depth: usize) {
    out.push_str("{\"opts\": [");
    for (i, o) in a.opts.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_json_str(out, o);
    }
    out.push_str("], \"dest\": ");
    write_json_str(out, a.dest);
    let nargs = view::nargs(a);
    let kind = match a.kind {
        Kind::Store => "store",
        Kind::StoreTrue => "store_true",
        Kind::StoreFalse => "store_false",
        Kind::StoreConst(_) if matches!(nargs, Nargs::Optional) => "store",
        Kind::StoreConst(_) => "store_const",
        Kind::Append => "append",
        Kind::Help => "help",
        Kind::Version => "version",
        Kind::SubParsers(_) => "parsers",
    };
    out.push_str(", \"action\": ");
    write_json_str(out, kind);
    out.push_str(", \"nargs\": ");
    match nargs {
        Nargs::One => out.push_str("null"),
        Nargs::Zero => out.push('0'),
        _ => write_json_str(out, view::nargs_str(a)),
    }
    out.push_str(", \"const\": ");
    match a.kind {
        Kind::StoreConst(c) => write_json_str(out, c),
        _ => out.push_str("null"),
    }
    out.push_str(", \"type\": ");
    match view::conv_name(a.conv) {
        "None" => out.push_str("null"),
        name => write_json_str(out, name),
    }
    out.push_str(", \"choices\": ");
    match a.choices {
        Some(c) => {
            out.push('[');
            for (i, s) in c.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json_str(out, s);
            }
            out.push(']');
        }
        None => out.push_str("null"),
    }
    out.push_str(", \"default\": ");
    match a.default {
        DefaultValue::None => out.push_str("{\"kind\": \"none\"}"),
        DefaultValue::Str(s) => {
            out.push_str("{\"kind\": \"str\", \"value\": ");
            write_json_str(out, s);
            out.push('}');
        }
        DefaultValue::Env { var, fallback } => {
            out.push_str("{\"kind\": \"env\", \"var\": ");
            write_json_str(out, var);
            out.push_str(", \"fallback\": ");
            write_json_str(out, fallback);
            out.push('}');
        }
        DefaultValue::False => out.push_str("{\"kind\": \"false\"}"),
        DefaultValue::True => out.push_str("{\"kind\": \"true\"}"),
        DefaultValue::Int(n) => {
            write!(out, "{{\"kind\": \"int\", \"value\": {n}}}").unwrap_or_default();
        }
        DefaultValue::EmptyList => out.push_str("{\"kind\": \"list\"}"),
        DefaultValue::Const(s) => {
            out.push_str("{\"kind\": \"const\", \"value\": ");
            write_json_str(out, s);
            out.push('}');
        }
    }
    out.push_str(", \"required\": ");
    bool_str(out, view::required(a));
    out.push_str(", \"help\": ");
    opt_str(out, a.help);
    out.push_str(", \"hidden\": ");
    bool_str(out, a.hidden);
    out.push_str(", \"metavar\": ");
    opt_str(out, view::metavar(a).as_deref());
    out.push_str(", \"mutex_group\": ");
    match a.mutex_group {
        Some(g) => write!(out, "{g}").unwrap_or_default(),
        None => out.push_str("null"),
    }
    out.push_str(", \"subcommands\": ");
    let subs = view::subcommands(a);
    if subs.is_empty() || depth >= MAX_DEPTH {
        out.push_str("null");
    } else {
        out.push('[');
        for (i, s) in subs.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str("{\"name\": ");
            write_json_str(out, s.name);
            out.push_str(", \"help\": ");
            opt_str(out, s.help);
            out.push_str(", \"listed\": ");
            bool_str(out, s.listed);
            out.push_str(", \"parser\": ");
            parser(out, s.spec, depth.saturating_add(1));
            out.push('}');
        }
        out.push(']');
    }
    out.push('}');
}
