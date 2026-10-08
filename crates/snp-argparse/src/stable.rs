//! [`crate::prefix_stable`]: extensions must not change what an existing command line means (12 §A.17 item 6,
//! ADR-057, ARCHITECTURE §9.2).
//!
//! argparse resolves `--abc` to the option strings that start with it (`_get_option_tuples`): one → that option,
//! several → `ambiguous option`. An extension option that shares a currently unique prefix turns a working command
//! (`pre-commit run --all`, `pre-commit install --conf x`) into a usage error, so every abbreviation `base` resolves
//! uniquely must resolve identically in `extended`.
//!
//! Owner: WP-12.

use crate::view::{self, MAX_DEPTH, View};
use crate::{ActionSpec, ParserSpec};

/// What an option string means: the action's dest, kind and nargs.
fn meaning(
    a: &ActionSpec,
) -> (
    Option<&'static str>,
    std::mem::Discriminant<crate::Kind>,
    String,
) {
    (
        view::dest(a),
        std::mem::discriminant(&a.kind),
        view::nargs_str(a).to_owned(),
    )
}

/// The option string that `token` (a `--` long-option prefix) resolves to uniquely, if any.
fn resolve(v: View, token: &str) -> Option<(&'static str, &'static ActionSpec)> {
    if let Some((i, o)) = v.exact(token.as_bytes()) {
        return v.action(i).map(|a| (o, a));
    }
    if !v.spec.allow_abbrev {
        return None;
    }
    let mut found = None;
    for (o, i) in v.options() {
        if o.starts_with(token) {
            if found.is_some() {
                return None;
            }
            found = v.action(i).map(|a| (o, a));
        }
    }
    found
}

pub(crate) fn check(
    base: &'static ParserSpec,
    extended: &'static ParserSpec,
) -> Result<(), String> {
    walk(base, extended, "", 0)
}

fn walk(
    base: &'static ParserSpec,
    extended: &'static ParserSpec,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    if depth >= MAX_DEPTH {
        return Ok(());
    }
    let b = View::new(base);
    let e = View::new(extended);
    let at = if path.is_empty() {
        "the top-level parser".to_owned()
    } else {
        format!("`{}`", path.trim_start())
    };
    if !b.negative_number_options() && e.negative_number_options() {
        return Err(format!(
            "{at}: a negative-number-like option string turns `-1` arguments into options"
        ));
    }
    for (option, i) in b.options() {
        let Some(base_action) = b.action(i) else {
            continue;
        };
        if let Some((ei, _)) = e.exact(option.as_bytes()) {
            let same = e
                .action(ei)
                .is_some_and(|ea| meaning(ea) == meaning(base_action));
            if !same {
                return Err(format!("{at}: option {option} changes meaning"));
            }
        } else {
            return Err(format!("{at}: option {option} is missing"));
        }
        if !option.starts_with("--") {
            continue;
        }
        // Every abbreviation `--x`, `--xy`, … of the long option.
        let chars: Vec<(usize, char)> = option.char_indices().collect();
        for (k, _) in chars.iter().skip(3) {
            check_prefix(b, e, option.get(..*k).unwrap_or_default(), &at)?;
        }
        check_prefix(b, e, option, &at)?;
    }
    for a in b.iter() {
        for sub in view::subcommands(a) {
            let Some(esub) = e.subcommand(sub.name.as_bytes()) else {
                return Err(format!("{at}: sub-command `{}` is missing", sub.name));
            };
            walk(
                sub.spec,
                esub.spec,
                &format!("{path} {}", sub.name),
                depth.saturating_add(1),
            )?;
        }
    }
    Ok(())
}

fn check_prefix(b: View, e: View, token: &str, at: &str) -> Result<(), String> {
    let Some((bo, ba)) = resolve(b, token) else {
        return Ok(());
    };
    match resolve(e, token) {
        Some((eo, ea)) if eo == bo && meaning(ea) == meaning(ba) => Ok(()),
        Some((eo, _)) => Err(format!(
            "{at}: `{token}` would resolve to {eo} instead of {bo}"
        )),
        None => Err(format!(
            "{at}: `{token}` (an abbreviation of {bo}) would become ambiguous"
        )),
    }
}
