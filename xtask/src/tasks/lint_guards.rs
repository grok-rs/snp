//! `cargo xtask lint-guards`: no `?` and no `return` inside `run`/`hook_impl` of crates/snp-run (§4.4, ADR-015).
//!
//! `snp_run::run` and `snp_run::hook_impl` have a single exit: the stash guard is restored explicitly before the
//! one return path, so neither body may contain the `?` operator or a `return` expression. Closures and nested
//! items are skipped (their `?`/`return` leave the closure or item, not the guarded function); macro bodies are
//! not expanded. Passes vacuously on the W0 stubs (`todo!()` bodies); fails if either function is missing.
//!
//! Owner: WP-00.

use std::ffi::OsString;

use anyhow::{Context as _, bail};
use syn::visit::{self, Visit};

use crate::workspace;

/// (file, function) pairs under the single-exit discipline.
const GUARDED: [(&str, &str); 2] = [
    ("crates/snp-run/src/run.rs", "run"),
    ("crates/snp-run/src/hook_impl.rs", "hook_impl"),
];

/// Counts early exits in one function body.
#[derive(Debug, Default)]
struct Exits {
    tries: usize,
    returns: usize,
}

impl<'ast> Visit<'ast> for Exits {
    fn visit_expr_try(&mut self, i: &'ast syn::ExprTry) {
        self.tries = self.tries.saturating_add(1);
        visit::visit_expr_try(self, i);
    }

    fn visit_expr_return(&mut self, i: &'ast syn::ExprReturn) {
        self.returns = self.returns.saturating_add(1);
        visit::visit_expr_return(self, i);
    }

    // A closure's `?`/`return` exits the closure; a nested item is a different function.
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_item(&mut self, _: &'ast syn::Item) {}
}

/// The violations of function `name` in `src`; `Err` when the function is missing.
pub(crate) fn check_source(src: &str, name: &str) -> anyhow::Result<Vec<String>> {
    let file = syn::parse_file(src).context("parsing Rust source")?;
    let f = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == name => Some(f),
            _ => None,
        })
        .with_context(|| format!("`fn {name}` not found"))?;
    let mut exits = Exits::default();
    for stmt in &f.block.stmts {
        exits.visit_stmt(stmt);
    }
    let mut v = Vec::new();
    if exits.tries > 0 {
        v.push(format!(
            "`fn {name}` contains {} `?` operator(s)",
            exits.tries
        ));
    }
    if exits.returns > 0 {
        v.push(format!(
            "`fn {name}` contains {} `return` expression(s)",
            exits.returns
        ));
    }
    Ok(v)
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    if let Some(a) = args.first() {
        bail!(
            "lint-guards takes no arguments (got {})",
            a.to_string_lossy()
        );
    }
    let root = workspace::root()?;
    let mut problems = Vec::new();
    for (file, name) in GUARDED {
        let path = root.join(file);
        let src = std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
        for v in check_source(&src, name).with_context(|| file.to_owned())? {
            problems.push(format!("{file}: {v}"));
        }
    }
    if problems.is_empty() {
        workspace::say("lint-guards: ok (snp_run::run and snp_run::hook_impl have a single exit)");
        return Ok(());
    }
    for p in &problems {
        workspace::say(&format!("lint-guards: {p}"));
    }
    bail!(
        "{} single-exit violation(s) (ARCHITECTURE §4.4)",
        problems.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_pass_vacuously() {
        assert_eq!(
            check_source("pub fn run(a: A) -> R { todo!() }", "run").unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn try_and_return_are_found() {
        let src = "fn run() -> R { let x = f()?; if x { return Err(e); } g()?; Ok(0) }";
        let v = check_source(src, "run").unwrap();
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v[0].contains("2 `?`"));
        assert!(v[1].contains("1 `return`"));
    }

    #[test]
    fn closures_and_nested_items_are_skipped() {
        let src = "fn hook_impl() -> R { let c = || { f()?; return Ok(1); }; fn inner() -> R { g()?; return x; } c() }";
        assert_eq!(
            check_source(src, "hook_impl").unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn missing_function_is_an_error() {
        assert!(check_source("fn other() {}", "run").is_err());
    }

    #[test]
    fn the_workspace_passes() {
        run(&[]).unwrap();
    }
}
