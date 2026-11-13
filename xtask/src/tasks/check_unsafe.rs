//! `cargo xtask check-unsafe`: no `unsafe` in snp code (S1, §11.6).
//!
//! Checks (all must hold):
//! * the root manifest sets `[workspace.lints.rust] unsafe_code = "forbid"`;
//! * every workspace member inherits it (`[lints] workspace = true`), and the fuzz crate forbids it too;
//! * no Rust source under `crates/`, `xtask/`, `fuzz/` or `proofs/` contains an `unsafe` block, fn, impl or trait,
//!   an `extern` block, an unsafe attribute (`#[unsafe(..)]`, `no_mangle`, `export_name`, `link_section`), the
//!   `unsafe` keyword inside a macro invocation, or an attribute that relaxes the `unsafe_code` lint;
//! * the Verus root `proofs/verus/kernels.rs`, which no Cargo manifest covers, carries `#![forbid(unsafe_code)]`
//!   itself (ADR-095: that also makes `assume_specification` impossible in the mounted kernel files). Only
//!   `proofs/verus/trusted/`, which does not exist in M1, would be exempt from these checks (docs/VERIFICATION.md §6).
//!
//! Owner: WP-00.

use std::ffi::OsString;
use std::path::Path;

use anyhow::{Context as _, bail};
use syn::buffer::{Cursor, TokenBuffer};
use syn::visit::{self, Visit};

use crate::workspace;

/// Directories whose Rust sources are scanned.
const SOURCE_DIRS: [&str; 4] = ["crates", "xtask", "fuzz", "proofs"];
/// Crate roots outside every Cargo manifest that must forbid `unsafe_code` themselves (ADR-095).
const STANDALONE_ROOTS: [&str; 1] = [super::proofs::ROOT];
/// Attributes that are unsafe in edition 2024.
const UNSAFE_ATTRS: [&str; 3] = ["no_mangle", "export_name", "link_section"];
/// Lint-level attributes that could relax `unsafe_code`.
const LEVEL_ATTRS: [&str; 4] = ["allow", "expect", "warn", "deny"];

/// Collects `unsafe` uses in one file (kinds only: line numbers would need proc-macro2's `span-locations`).
#[derive(Debug, Default)]
struct Finder {
    hits: Vec<String>,
}

impl Finder {
    fn hit(&mut self, what: &str) {
        self.hits.push(what.to_owned());
    }

    /// The `unsafe` keyword anywhere in a macro's token stream (macros are not expanded).
    fn scan_tokens(&mut self, cursor: Cursor<'_>) {
        let mut c = cursor;
        while !c.eof() {
            if let Some((inside, _, _, next)) = c.any_group() {
                self.scan_tokens(inside);
                c = next;
            } else if let Some((ident, next)) = c.ident() {
                if ident == "unsafe" {
                    self.hit("`unsafe` inside a macro invocation");
                }
                c = next;
            } else if let Some((_, next)) = c.token_tree() {
                c = next;
            } else {
                break;
            }
        }
    }
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_expr_unsafe(&mut self, i: &'ast syn::ExprUnsafe) {
        self.hit("`unsafe` block");
        visit::visit_expr_unsafe(self, i);
    }

    fn visit_signature(&mut self, i: &'ast syn::Signature) {
        if i.unsafety.is_some() {
            self.hit("`unsafe fn`");
        }
        visit::visit_signature(self, i);
    }

    fn visit_type_bare_fn(&mut self, i: &'ast syn::TypeBareFn) {
        if i.unsafety.is_some() {
            self.hit("`unsafe fn` pointer type");
        }
        visit::visit_type_bare_fn(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if i.unsafety.is_some() {
            self.hit("`unsafe impl`");
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        if i.unsafety.is_some() {
            self.hit("`unsafe trait`");
        }
        visit::visit_item_trait(self, i);
    }

    fn visit_item_foreign_mod(&mut self, i: &'ast syn::ItemForeignMod) {
        self.hit("`extern` block");
        visit::visit_item_foreign_mod(self, i);
    }

    fn visit_attribute(&mut self, i: &'ast syn::Attribute) {
        let path = i.path();
        if path.is_ident("unsafe") {
            self.hit("unsafe attribute `#[unsafe(..)]`");
        } else if UNSAFE_ATTRS.iter().any(|a| path.is_ident(a)) {
            self.hit("unsafe attribute");
        } else if LEVEL_ATTRS.iter().any(|a| path.is_ident(a))
            && let syn::Meta::List(list) = &i.meta
            && list.tokens.to_string().contains("unsafe_code")
        {
            self.hit("attribute relaxing `unsafe_code`");
        }
        visit::visit_attribute(self, i);
    }

    fn visit_macro(&mut self, i: &'ast syn::Macro) {
        let buf = TokenBuffer::new2(i.tokens.clone());
        self.scan_tokens(buf.begin());
        visit::visit_macro(self, i);
    }
}

/// Every `unsafe` use in `src` (one entry per use).
pub(crate) fn scan_source(src: &str) -> anyhow::Result<Vec<String>> {
    let file = syn::parse_file(src).context("parsing Rust source")?;
    let mut f = Finder::default();
    f.visit_file(&file);
    Ok(f.hits)
}

/// Whether `src` carries the inner attribute `#![forbid(unsafe_code)]` (possibly among other lints).
fn forbids_unsafe(src: &str) -> anyhow::Result<bool> {
    let file = syn::parse_file(src).context("parsing Rust source")?;
    Ok(file.attrs.iter().any(|a| {
        matches!(a.style, syn::AttrStyle::Inner(_))
            && matches!(&a.meta, syn::Meta::List(l)
                if l.path.is_ident("forbid") && l.tokens.to_string().contains("unsafe_code"))
    }))
}

/// `[lints] workspace = true` in a member manifest.
fn inherits_workspace_lints(manifest: &toml::Table) -> bool {
    manifest
        .get("lints")
        .and_then(|l| l.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}

fn read_toml(path: &Path) -> anyhow::Result<toml::Table> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    text.parse::<toml::Table>()
        .with_context(|| format!("parsing {}", path.display()))
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    if let Some(a) = args.first() {
        bail!(
            "check-unsafe takes no arguments (got {})",
            a.to_string_lossy()
        );
    }
    let root = workspace::root()?;
    let mut problems = Vec::new();

    let root_manifest = read_toml(&root.join("Cargo.toml"))?;
    let forbid = root_manifest
        .get("workspace")
        .and_then(|w| w.get("lints"))
        .and_then(|l| l.get("rust"))
        .and_then(|r| r.get("unsafe_code"))
        .and_then(toml::Value::as_str);
    if forbid != Some("forbid") {
        problems
            .push("Cargo.toml: [workspace.lints.rust] unsafe_code must be \"forbid\"".to_owned());
    }

    let meta = workspace::metadata(&root)?;
    for p in meta
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(manifest) = p.get("manifest_path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let manifest = Path::new(manifest);
        if !inherits_workspace_lints(&read_toml(manifest)?) {
            problems.push(format!(
                "{}: missing `[lints] workspace = true`",
                workspace::rel(&root, manifest)
            ));
        }
    }
    let fuzz = root.join("fuzz/Cargo.toml");
    if fuzz.is_file() {
        let t = read_toml(&fuzz)?;
        let level = t
            .get("lints")
            .and_then(|l| l.get("rust"))
            .and_then(|r| r.get("unsafe_code"))
            .and_then(toml::Value::as_str);
        if level != Some("forbid") {
            problems
                .push("fuzz/Cargo.toml: [lints.rust] unsafe_code must be \"forbid\"".to_owned());
        }
    }

    for rel in STANDALONE_ROOTS {
        let path = root.join(rel);
        if !path.is_file() {
            continue;
        }
        let src = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
        if !forbids_unsafe(&src).with_context(|| rel.to_owned())? {
            problems.push(format!(
                "{rel}: a standalone root must carry `#![forbid(unsafe_code)]`"
            ));
        }
    }

    let trusted = root.join(super::proofs::TRUSTED_DIR);
    let mut files = 0_usize;
    for dir in SOURCE_DIRS {
        for path in workspace::rust_files(&root.join(dir))? {
            if path.starts_with(&trusted) {
                continue;
            }
            files = files.saturating_add(1);
            let src = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let hits = scan_source(&src).with_context(|| workspace::rel(&root, &path))?;
            for what in hits {
                problems.push(format!("{}: {what}", workspace::rel(&root, &path)));
            }
        }
    }

    if problems.is_empty() {
        workspace::say(&format!(
            "check-unsafe: ok ({files} Rust files, every crate forbids unsafe_code)"
        ));
        return Ok(());
    }
    for p in &problems {
        workspace::say(&format!("check-unsafe: {p}"));
    }
    bail!("{} unsafe finding(s)", problems.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whats(src: &str) -> Vec<String> {
        scan_source(src).unwrap()
    }

    #[test]
    fn clean_source_passes() {
        let src = "//! unsafe in a comment\nfn f() -> &'static str { \"unsafe\" }\n#[expect(dead_code, reason = \"x\")]\nfn g() { println!(\"unsafe {}\", 1); }\n";
        assert_eq!(whats(src), Vec::<String>::new());
    }

    #[test]
    fn every_form_is_found() {
        let src = r#"
            fn a() { unsafe { std::hint::unreachable_unchecked() } }
            unsafe fn b() {}
            unsafe impl Send for X {}
            unsafe trait T {}
            unsafe extern "C" { fn c(); }
            #[unsafe(no_mangle)] fn d() {}
            #[no_mangle] fn e() {}
            #[allow(unsafe_code)] fn f() {}
            type P = unsafe fn();
            m!(unsafe { 1 });
        "#;
        let w = whats(src);
        assert_eq!(w.len(), 10, "{w:?}");
    }

    #[test]
    fn standalone_roots_must_forbid_unsafe() {
        assert!(forbids_unsafe("#![forbid(unsafe_code)]\nfn f() {}").unwrap());
        assert!(forbids_unsafe("#![forbid(missing_docs, unsafe_code)]\n").unwrap());
        assert!(!forbids_unsafe("#![deny(unsafe_code)]\n").unwrap());
        assert!(!forbids_unsafe("#[forbid(unsafe_code)]\nfn f() {}").unwrap());
    }

    #[test]
    fn the_workspace_is_clean() {
        run(&[]).unwrap();
    }
}
