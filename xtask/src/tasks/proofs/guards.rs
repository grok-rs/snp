//! The tool-free guards G1-G10 of docs/VERIFICATION.md §3.2 (`cargo xtask proofs check`).
//!
//! They make "the verified file is the shipped file, and only ghost code differs" a mechanical fact:
//!
//! | ID | Check |
//! |---|---|
//! | G1 | registry, kernel markers and root mounts agree both ways; mounts carry a bare `#[verus_verify]` and a `#[path]` under `crates/`; the root forbids `unsafe_code` |
//! | G2 | the only cfg predicates in a kernel file are a bare `verus_keep_ghost`, and `test` on a `mod`; no `cfg!` |
//! | G3 | `#[cfg(verus_keep_ghost)]` only on `use vstd::prelude::*`, `use crate::spec::…` and `proof!`/`proof_decl!` statements; no `verus!` |
//! | G4 | `cfg_attr(verus_keep_ghost, X)`: X on the K4 allowlist (`verus_spec`, bare `verus_verify`, `verifier::{external_derive, rlimit, loop_isolation, spinoff_prover}`) |
//! | G5 | no cheat token (`assume`, `admit`, `assume_specification`, `external_body`, `external`, `exec_allows_no_decreases_clause`) in kernel files, the root or the specs |
//! | G6 | no `requires` in the contract of a non-private function (trait method declarations exempt) |
//! | G7 | `reason = "verus: …"` expectations only on functions of mounted kernel files |
//! | G8 | every impl of a kernel trait outside kernel files is a `[[trusted]]` entry whose named test exists |
//! | G9 | exec code in a kernel file names no path rooted in `crate`, `super`, `self::super` or an external crate |
//! | G10 | every caller exists and calls the kernel; every model file, oracle and anchor exists; every anchor fingerprint matches |
//!
//! Tokens are compared after syn's lexing, so comments and string contents never trigger a guard; macro bodies
//! (which syn does not parse) are scanned token by token.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use anyhow::Context as _;
use syn::buffer::{Cursor, TokenBuffer};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};

use super::fingerprint;
use super::{LEAN_DIR, REGISTRY, ROOT, Registry, SPEC_DIR, Status, Tree};

/// The marker line of a verified kernel file (rule K1).
pub(crate) const MARKER: &str = "//! Verified kernel (ADR-095):";
/// The marker line of a kernel whose proof is pending (§8.3).
pub(crate) const PENDING_MARKER: &str = "//! Kernel (ADR-095), proof pending: #";
/// Identifiers that weaken a proof; `--no-cheating` does not catch all of them (tested).
const CHEATS: [&str; 6] = [
    "assume",
    "admit",
    "assume_specification",
    "external_body",
    "external",
    "exec_allows_no_decreases_clause",
];
/// Path roots exec code may use besides the file's own items (rule K2).
const STD_ROOTS: [&str; 5] = ["std", "core", "alloc", "self", "Self"];
/// Names of the std prelude and the primitive types: never an external crate.
const PRELUDE: [&str; 51] = [
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "Vec",
    "String",
    "Box",
    "ToOwned",
    "ToString",
    "Clone",
    "Copy",
    "Default",
    "Drop",
    "Eq",
    "PartialEq",
    "Ord",
    "PartialOrd",
    "Iterator",
    "IntoIterator",
    "DoubleEndedIterator",
    "ExactSizeIterator",
    "Extend",
    "FromIterator",
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "AsRef",
    "AsMut",
    "Fn",
    "FnMut",
    "FnOnce",
    "Send",
    "Sync",
    "Sized",
    "bool",
    "char",
    "str",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "isize",
];
/// Module mounts of the root that are not kernel files.
const ROOT_MODULES: [&str; 2] = ["spec", "trusted"];

/// One guard violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    /// `G1` … `G10`.
    pub(crate) guard: &'static str,
    /// The file it is about (repo-relative).
    pub(crate) file: String,
    pub(crate) msg: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.guard, self.file, self.msg)
    }
}

/// Collects problems.
#[derive(Debug, Default)]
struct Report {
    problems: Vec<Problem>,
}

impl Report {
    fn add(&mut self, guard: &'static str, file: &str, msg: impl Into<String>) {
        self.problems.push(Problem {
            guard,
            file: file.to_owned(),
            msg: msg.into(),
        });
    }
}

/// Runs G1-G10 over `tree`.
pub(crate) fn check(tree: &Tree, reg: &Registry) -> anyhow::Result<Vec<Problem>> {
    let mut r = Report::default();
    g1(tree, reg, &mut r)?;
    for k in &reg.kernels {
        let Some(src) = tree.read(&k.file) else {
            continue; // G1 reports it
        };
        match syn::parse_file(&src) {
            Ok(file) => scan_kernel(&k.file, &file, &mut r),
            Err(e) => r.add("G1", &k.file, format!("does not parse: {e}")),
        }
        cheats(&k.file, &src, &mut r);
    }
    if let Some(src) = tree.read(ROOT) {
        cheats(ROOT, &src, &mut r);
    }
    for f in tree.rust_files(SPEC_DIR)? {
        if let Some(src) = tree.read(&f) {
            cheats(&f, &src, &mut r);
        }
    }
    g7(tree, reg, &mut r)?;
    g8(tree, reg, &mut r)?;
    g10(tree, reg, &mut r);
    Ok(r.problems)
}

// ---------------------------------------------------------------------------------------------------------------
// G1: registry ↔ markers ↔ mounts

/// One `mod` item of the root.
#[derive(Debug)]
struct Mount {
    name: String,
    path: Option<String>,
    verus_verify: bool,
}

fn g1(tree: &Tree, reg: &Registry, r: &mut Report) -> anyhow::Result<()> {
    let mounts = match tree.read(ROOT) {
        None => {
            r.add("G1", ROOT, "the Verus root is missing");
            Vec::new()
        }
        Some(src) => match syn::parse_file(&src) {
            Err(e) => {
                r.add("G1", ROOT, format!("does not parse: {e}"));
                Vec::new()
            }
            Ok(file) => {
                if !file.attrs.iter().any(is_forbid_unsafe) {
                    r.add("G1", ROOT, "the root must carry `#![forbid(unsafe_code)]`");
                }
                root_mounts(&file)
            }
        },
    };

    let mut seen: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for k in &reg.kernels {
        for key in [
            ("id", k.id.as_str()),
            ("file", &k.file),
            ("mount", &k.mount),
        ] {
            let n = seen.entry(key).or_default();
            *n = n.saturating_add(1);
        }
    }
    for ((what, value), n) in &seen {
        if *n > 1 {
            r.add(
                "G1",
                REGISTRY,
                format!("{what} {value:?} appears in {n} [[kernel]] entries"),
            );
        }
    }

    for k in &reg.kernels {
        if !k.file.starts_with("crates/")
            || Path::new(&k.file).extension().is_none_or(|e| e != "rs")
        {
            r.add(
                "G1",
                REGISTRY,
                format!(
                    "{}: file {:?} must be a .rs file under crates/",
                    k.id, k.file
                ),
            );
        }
        if !tree.is_file(&k.spec) {
            r.add(
                "G1",
                REGISTRY,
                format!("{}: spec {} does not exist", k.id, k.spec),
            );
        }
        if k.callers.is_empty() {
            r.add(
                "G1",
                REGISTRY,
                format!("{}: callers must name at least one production caller", k.id),
            );
        }
        if k.status == Status::Verified && k.mutants.is_empty() {
            r.add(
                "G1",
                REGISTRY,
                format!("{}: a verified kernel needs at least one mutant (V3)", k.id),
            );
        }
        match tree.read(&k.file) {
            None => r.add(
                "G1",
                &k.file,
                format!("kernel {} names a file that does not exist", k.id),
            ),
            Some(src) => {
                let first = src.lines().next().unwrap_or_default();
                let (want, label) = match k.status {
                    Status::Verified => (MARKER, "verified"),
                    Status::Pending => (PENDING_MARKER, "pending"),
                };
                if !first.starts_with(want) {
                    r.add(
                        "G1",
                        &k.file,
                        format!(
                            "a {label} kernel's first line must start with `{want}` (rule K1, §8.3)"
                        ),
                    );
                }
            }
        }
        let mine: Vec<&Mount> = mounts.iter().filter(|m| m.name == k.mount).collect();
        match k.status {
            Status::Verified => {
                let want = format!("../../{}", k.file);
                if mine.len() != 1 {
                    r.add(
                        "G1",
                        ROOT,
                        format!(
                            "kernel {} needs exactly one mount `{}`, found {}",
                            k.id,
                            k.mount,
                            mine.len()
                        ),
                    );
                }
                for m in mine {
                    if m.path.as_deref() != Some(want.as_str()) {
                        r.add(
                            "G1",
                            ROOT,
                            format!(
                                "mount `{}` must carry #[path = \"{want}\"] (kernel {})",
                                m.name, k.id
                            ),
                        );
                    }
                }
            }
            Status::Pending => {
                if !mine.is_empty() {
                    r.add(
                        "G1",
                        ROOT,
                        format!("pending kernel {} must not be mounted (§8.3)", k.id),
                    );
                }
            }
        }
    }

    for m in &mounts {
        if m.path.is_none() {
            if !ROOT_MODULES.contains(&m.name.as_str()) {
                r.add(
                    "G1",
                    ROOT,
                    format!(
                        "`mod {}` is neither a kernel mount (#[path]) nor spec/trusted",
                        m.name
                    ),
                );
            }
            continue;
        }
        let registered = reg
            .kernels
            .iter()
            .any(|k| k.status == Status::Verified && k.mount == m.name);
        if !registered {
            r.add(
                "G1",
                ROOT,
                format!(
                    "mount `{}` has no verified [[kernel]] entry in {REGISTRY}",
                    m.name
                ),
            );
        }
        if !m.verus_verify {
            r.add(
                "G1",
                ROOT,
                format!("mount `{}` must carry a bare #[verus_verify] (otherwise Verus skips unannotated functions)", m.name),
            );
        }
        if !m
            .path
            .as_deref()
            .is_some_and(|p| p.starts_with("../../crates/"))
        {
            r.add(
                "G1",
                ROOT,
                format!(
                    "mount `{}` must point under crates/ (#[path = \"../../crates/…\"])",
                    m.name
                ),
            );
        }
    }

    for f in tree.rust_files("crates")? {
        let Some(src) = tree.read(&f) else { continue };
        let first = src.lines().next().unwrap_or_default();
        let marked = first.starts_with("//! Verified kernel (ADR-095)")
            || first.starts_with("//! Kernel (ADR-095)");
        if marked && reg.kernel_of_file(&f).is_none() {
            r.add(
                "G1",
                &f,
                format!("carries a kernel marker but has no [[kernel]] entry in {REGISTRY}"),
            );
        }
    }
    Ok(())
}

fn root_mounts(file: &syn::File) -> Vec<Mount> {
    file.items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(m) => Some(Mount {
                name: m.ident.to_string(),
                path: m.attrs.iter().find_map(path_attr),
                verus_verify: m
                    .attrs
                    .iter()
                    .any(|a| matches!(&a.meta, syn::Meta::Path(p) if p.is_ident("verus_verify"))),
            }),
            _ => None,
        })
        .collect()
}

/// The value of `#[path = "…"]`.
fn path_attr(a: &syn::Attribute) -> Option<String> {
    match &a.meta {
        syn::Meta::NameValue(nv) if nv.path.is_ident("path") => match &nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

/// `#![forbid(unsafe_code)]` (possibly among other lints).
pub(crate) fn is_forbid_unsafe(a: &syn::Attribute) -> bool {
    matches!(a.style, syn::AttrStyle::Inner(_))
        && matches!(&a.meta, syn::Meta::List(l) if l.path.is_ident("forbid") && has_ident(&TokenBuffer::new2(l.tokens.clone()), "unsafe_code"))
}

// ---------------------------------------------------------------------------------------------------------------
// G2, G3, G4, G6, G7 (kernel side), G9: one walk over a kernel file

/// What an attribute is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// A `use` of `vstd::prelude::*` or `crate::spec::…`.
    GhostUse,
    /// Any other `use`.
    OtherUse,
    /// A `mod` item.
    Mod,
    /// A `proof!`/`proof_decl!` statement.
    ProofStmt,
    /// A function (`public`: non-private and not in a trait impl).
    Fn { public: bool },
    /// A trait method declaration.
    TraitFn,
    /// Anything else.
    Other,
}

/// Walks one kernel file.
#[derive(Debug)]
struct KernelScan<'r> {
    file: String,
    /// Attribute address → owner, recorded by the item/statement visitors before their attributes are visited.
    owners: BTreeMap<usize, Owner>,
    /// Names the file declares or imports (for G9).
    locals: BTreeSet<String>,
    /// Names the file declares itself (items and generic parameters, no imports).
    items: BTreeSet<String>,
    in_trait_impl: bool,
    report: &'r mut Report,
}

fn addr(a: &syn::Attribute) -> usize {
    std::ptr::from_ref(a).addr()
}

/// Scans a parsed kernel file (G2, G3, G4, G6, G7, G9).
fn scan_kernel(rel: &str, file: &syn::File, r: &mut Report) {
    let mut locals = Locals::default();
    locals.visit_file(file);
    let mut s = KernelScan {
        file: rel.to_owned(),
        owners: BTreeMap::new(),
        items: locals.items,
        locals: locals.names,
        in_trait_impl: false,
        report: r,
    };
    s.visit_file(file);
}

impl KernelScan<'_> {
    /// Whether a `use` tree's root stays inside the file and std (an imported name cannot justify its own import).
    fn use_root_ok(&self, root: &str) -> bool {
        STD_ROOTS.contains(&root) || self.items.contains(root)
    }

    /// Whether a path starting with `first` (then `second`) stays inside the file and std.
    fn root_ok(&self, first: &str, second: Option<&str>) -> bool {
        match first {
            "crate" | "super" => false,
            "self" => second != Some("super"),
            f => STD_ROOTS.contains(&f) || PRELUDE.contains(&f) || self.locals.contains(f),
        }
    }

    fn add(&mut self, guard: &'static str, msg: impl Into<String>) {
        let file = self.file.clone();
        self.report.add(guard, &file, msg);
    }

    fn classify(&mut self, attrs: &[syn::Attribute], owner: Owner) {
        for a in attrs {
            self.owners.insert(addr(a), owner);
        }
    }

    fn check_cfg(&mut self, a: &syn::Attribute, owner: Owner) {
        match a.parse_args::<syn::Meta>() {
            Ok(syn::Meta::Path(p)) if p.is_ident("verus_keep_ghost") => match owner {
                Owner::GhostUse | Owner::ProofStmt => {}
                Owner::OtherUse => self.add(
                    "G3",
                    "a #[cfg(verus_keep_ghost)] `use` may import only `vstd::prelude::*` or `crate::spec::…`",
                ),
                _ => self.add(
                    "G3",
                    "#[cfg(verus_keep_ghost)] may only mark a `use` of vstd::prelude/crate::spec or a proof!/proof_decl! statement",
                ),
            },
            Ok(syn::Meta::Path(p)) if p.is_ident("test") && owner == Owner::Mod => {}
            Ok(m) => self.add(
                "G2",
                format!("cfg predicate `{}`: only a bare `verus_keep_ghost` (and `test` on a mod) is allowed", meta_str(&m)),
            ),
            Err(e) => self.add("G2", format!("unparsable #[cfg]: {e}")),
        }
    }

    fn check_cfg_attr(&mut self, a: &syn::Attribute, owner: Owner) {
        let metas =
            match a.parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated) {
                Ok(m) => m,
                Err(e) => {
                    self.add("G4", format!("unparsable #[cfg_attr]: {e}"));
                    return;
                }
            };
        let mut it = metas.iter();
        match it.next() {
            Some(syn::Meta::Path(p)) if p.is_ident("verus_keep_ghost") => {}
            Some(m) => {
                self.add(
                    "G2",
                    format!(
                        "cfg_attr predicate `{}`: only a bare `verus_keep_ghost` is allowed",
                        meta_str(m)
                    ),
                );
                return;
            }
            None => {
                self.add("G4", "empty #[cfg_attr]");
                return;
            }
        }
        let mut any = false;
        for m in it {
            any = true;
            self.check_payload(m, owner);
        }
        if !any {
            self.add("G4", "cfg_attr(verus_keep_ghost) without an attribute");
        }
    }

    /// One payload of `cfg_attr(verus_keep_ghost, …)` (G4, G6).
    fn check_payload(&mut self, m: &syn::Meta, owner: Owner) {
        let path = path_str(m.path());
        let ok = match (path.as_str(), m) {
            ("verus_spec", syn::Meta::List(l)) => {
                if owner == (Owner::Fn { public: true })
                    && has_ident(&TokenBuffer::new2(l.tokens.clone()), "requires")
                {
                    self.add(
                        "G6",
                        "a non-private function's contract has `requires`: an unverified caller would silently assume it (rule K5)",
                    );
                }
                true
            }
            ("verus_verify", syn::Meta::List(_)) => {
                self.add("G4", "`verus_verify(...)` with arguments is not allowed (it can mark items external)");
                return;
            }
            ("verifier::external_derive", syn::Meta::Path(_) | syn::Meta::List(_))
            | ("verifier::rlimit" | "verifier::loop_isolation", syn::Meta::List(_))
            | ("verifier::spinoff_prover" | "verus_spec" | "verus_verify", syn::Meta::Path(_)) => {
                true
            }
            _ => false,
        };
        if !ok {
            let cheat = m
                .path()
                .segments
                .iter()
                .any(|s| CHEATS.contains(&s.ident.to_string().as_str()));
            let what = if cheat {
                " (a cheat: --no-cheating does not reject it)"
            } else {
                ""
            };
            self.add(
                "G4",
                format!(
                    "`cfg_attr(verus_keep_ghost, {})` is not on the K4 allowlist (verus_spec, verus_verify, \
                     verifier::{{external_derive, rlimit, loop_isolation, spinoff_prover}}){what}",
                    meta_str(m)
                ),
            );
        }
    }
}

impl<'ast> Visit<'ast> for KernelScan<'_> {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        let item = i;
        match item {
            syn::Item::Use(u) => {
                let ghost = u.attrs.iter().any(is_ghost_cfg);
                let owner = if ghost && allowed_ghost_use(&u.tree) {
                    Owner::GhostUse
                } else {
                    Owner::OtherUse
                };
                self.classify(&u.attrs, owner);
                for a in &u.attrs {
                    self.visit_attribute(a);
                }
                if !ghost {
                    let mut roots = Vec::new();
                    use_roots(&u.tree, u.leading_colon.is_some(), &mut roots);
                    for root in roots {
                        if !self.use_root_ok(&root) {
                            self.add(
                                "G9",
                                format!("`use {root}…` reaches outside the file and std (rule K2: kernel files are self-contained)"),
                            );
                        }
                    }
                }
            }
            syn::Item::Mod(m) => {
                self.classify(&m.attrs, Owner::Mod);
                if m.attrs.iter().any(is_test_cfg) {
                    for a in &m.attrs {
                        self.visit_attribute(a);
                    }
                    return; // unit tests are neither verified nor shipped
                }
                visit::visit_item_mod(self, m);
            }
            syn::Item::Fn(f) => {
                let public = !matches!(f.vis, syn::Visibility::Inherited);
                self.classify(&f.attrs, Owner::Fn { public });
                visit::visit_item_fn(self, f);
            }
            syn::Item::Impl(i) => {
                self.classify(&i.attrs, Owner::Other);
                let saved = self.in_trait_impl;
                self.in_trait_impl = i.trait_.is_some();
                visit::visit_item_impl(self, i);
                self.in_trait_impl = saved;
            }
            _ => visit::visit_item(self, item),
        }
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        let f = i;
        let public = !self.in_trait_impl && !matches!(f.vis, syn::Visibility::Inherited);
        self.classify(&f.attrs, Owner::Fn { public });
        visit::visit_impl_item_fn(self, f);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        let f = i;
        self.classify(&f.attrs, Owner::TraitFn);
        visit::visit_trait_item_fn(self, f);
    }

    fn visit_stmt(&mut self, i: &'ast syn::Stmt) {
        let s = i;
        if let syn::Stmt::Macro(m) = s
            && is_proof_macro(&m.mac)
        {
            self.classify(&m.attrs, Owner::ProofStmt);
            for a in &m.attrs {
                self.visit_attribute(a);
            }
            if !m.attrs.iter().any(is_ghost_cfg) {
                self.add(
                    "G3",
                    "a proof!/proof_decl! statement must carry #[cfg(verus_keep_ghost)]",
                );
            }
            return; // ghost code: never compiled by rustc
        }
        visit::visit_stmt(self, s);
    }

    fn visit_expr_macro(&mut self, i: &'ast syn::ExprMacro) {
        let m = i;
        if is_proof_macro(&m.mac) {
            self.add(
                "G3",
                "proof!/proof_decl! must be a statement carrying #[cfg(verus_keep_ghost)]",
            );
            return;
        }
        visit::visit_expr_macro(self, m);
    }

    fn visit_attribute(&mut self, i: &'ast syn::Attribute) {
        let a = i;
        let owner = self.owners.get(&addr(a)).copied().unwrap_or(Owner::Other);
        let path = a.path();
        if path.is_ident("cfg") {
            self.check_cfg(a, owner);
        } else if path.is_ident("cfg_attr") {
            self.check_cfg_attr(a, owner);
        } else if path.is_ident("expect") || path.is_ident("allow") {
            if let syn::Meta::List(l) = &a.meta
                && has_verus_reason(&TokenBuffer::new2(l.tokens.clone()))
            {
                if path.is_ident("allow") {
                    self.add(
                        "G7",
                        "a `reason = \"verus: …\"` lint relief must be #[expect], not #[allow]",
                    );
                } else if !matches!(owner, Owner::Fn { .. }) {
                    self.add("G7", "a `reason = \"verus: …\"` expectation belongs on a verified function (rule K7)");
                }
            }
        } else if path
            .segments
            .first()
            .is_some_and(|s| s.ident == "verifier" || s.ident.to_string().starts_with("verus"))
        {
            self.add(
                "G4",
                format!(
                    "`#[{}]` outside cfg_attr(verus_keep_ghost, …) (ghost forms of rule K4 only)",
                    path_str(path)
                ),
            );
        }
        // Attributes are not exec code: no descent (G9).
    }

    fn visit_macro(&mut self, i: &'ast syn::Macro) {
        let m = i;
        let name = m
            .path
            .segments
            .last()
            .map_or_default(|s| s.ident.to_string());
        if name == "verus" {
            self.add("G3", "kernel files contain no verus! block: specs and lemmas go into proofs/verus/spec/ (rule K4)");
            return;
        }
        if name == "cfg" {
            self.add("G2", "`cfg!` in a kernel file: platform behaviour is a `windows: bool` parameter (rule K3)");
        }
        let buf = TokenBuffer::new2(m.tokens.clone());
        let mut found = Vec::new();
        macro_tokens(buf.begin(), &mut found);
        for t in found {
            match t {
                MacroToken::Cfg => {
                    self.add("G2", "`cfg!` inside a macro of a kernel file (rule K3)");
                }
                MacroToken::Root(w) => self.add(
                    "G9",
                    format!("`{w}` path inside a macro of exec code (rule K2)"),
                ),
            }
        }
        visit::visit_macro(self, m);
    }

    fn visit_path(&mut self, i: &'ast syn::Path) {
        let p = i;
        let mut segs = p.segments.iter();
        if let Some(first) = segs.next() {
            let first = first.ident.to_string();
            let second = segs.next().map(|s| s.ident.to_string());
            let rooted = p.leading_colon.is_some() || second.is_some();
            if rooted && !self.root_ok(&first, second.as_deref())
                || (p.leading_colon.is_some()
                    && !matches!(first.as_str(), "std" | "core" | "alloc"))
            {
                let shown = path_str(p);
                self.add(
                    "G9",
                    format!("path `{shown}` in exec code: only std/core/alloc and the file's own items (rule K2)"),
                );
            }
        }
        visit::visit_path(self, p);
    }
}

/// The first segment of every path of a `use` tree.
fn use_roots(tree: &syn::UseTree, leading_colon: bool, out: &mut Vec<String>) {
    let lead = if leading_colon { "::" } else { "" };
    match tree {
        syn::UseTree::Path(p) => out.push(format!("{lead}{}", p.ident)),
        syn::UseTree::Name(n) => out.push(format!("{lead}{}", n.ident)),
        syn::UseTree::Rename(r) => out.push(format!("{lead}{}", r.ident)),
        syn::UseTree::Glob(_) => out.push(format!("{lead}*")),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                use_roots(t, leading_colon, out);
            }
        }
    }
}

/// `vstd::prelude::*` or `crate::spec::…`.
fn allowed_ghost_use(tree: &syn::UseTree) -> bool {
    let mut segs = Vec::new();
    let mut t = tree;
    loop {
        match t {
            syn::UseTree::Path(p) => {
                segs.push(p.ident.to_string());
                t = &p.tree;
            }
            syn::UseTree::Glob(_) => {
                segs.push("*".to_owned());
                break;
            }
            syn::UseTree::Name(n) => {
                segs.push(n.ident.to_string());
                break;
            }
            _ => return false,
        }
    }
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    matches!(
        s.as_slice(),
        ["vstd", "prelude", "*"] | ["crate", "spec", _, ..]
    )
}

fn is_ghost_cfg(a: &syn::Attribute) -> bool {
    a.path().is_ident("cfg")
        && matches!(a.parse_args::<syn::Meta>(), Ok(syn::Meta::Path(p)) if p.is_ident("verus_keep_ghost"))
}

fn is_test_cfg(a: &syn::Attribute) -> bool {
    a.path().is_ident("cfg")
        && matches!(a.parse_args::<syn::Meta>(), Ok(syn::Meta::Path(p)) if p.is_ident("test"))
}

fn is_proof_macro(m: &syn::Macro) -> bool {
    m.path.is_ident("proof") || m.path.is_ident("proof_decl")
}

/// Findings inside a macro's tokens.
#[derive(Debug)]
enum MacroToken {
    Cfg,
    Root(String),
}

fn macro_tokens(start: Cursor<'_>, out: &mut Vec<MacroToken>) {
    let mut c = start;
    while !c.eof() {
        if let Some((inside, _, _, next)) = c.any_group() {
            macro_tokens(inside, out);
            c = next;
        } else if let Some((ident, next)) = c.ident() {
            if ident == "crate" || ident == "super" {
                out.push(MacroToken::Root(ident.to_string()));
            } else if ident == "cfg" && next.punct().is_some_and(|(p, _)| p.as_char() == '!') {
                out.push(MacroToken::Cfg);
            }
            c = next;
        } else if let Some((_, next)) = c.token_tree() {
            c = next;
        } else {
            break;
        }
    }
}

/// Names a kernel file declares or imports (items at any depth, `use` leaves, generic parameters).
#[derive(Debug, Default)]
struct Locals {
    names: BTreeSet<String>,
    items: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Locals {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        let item = i;
        let ident = match item {
            syn::Item::Const(i) => Some(&i.ident),
            syn::Item::Enum(i) => Some(&i.ident),
            syn::Item::Fn(i) => Some(&i.sig.ident),
            syn::Item::Mod(i) => Some(&i.ident),
            syn::Item::Static(i) => Some(&i.ident),
            syn::Item::Struct(i) => Some(&i.ident),
            syn::Item::Trait(i) => Some(&i.ident),
            syn::Item::Type(i) => Some(&i.ident),
            syn::Item::Union(i) => Some(&i.ident),
            syn::Item::Macro(i) => i.ident.as_ref(),
            _ => None,
        };
        if let Some(i) = ident {
            self.names.insert(i.to_string());
            self.items.insert(i.to_string());
        }
        visit::visit_item(self, item);
    }

    fn visit_use_name(&mut self, i: &'ast syn::UseName) {
        let n = i;
        self.names.insert(n.ident.to_string());
    }

    fn visit_use_rename(&mut self, i: &'ast syn::UseRename) {
        let r = i;
        self.names.insert(r.rename.to_string());
    }

    fn visit_type_param(&mut self, i: &'ast syn::TypeParam) {
        let p = i;
        self.names.insert(p.ident.to_string());
        self.items.insert(p.ident.to_string());
        visit::visit_type_param(self, p);
    }

    fn visit_const_param(&mut self, i: &'ast syn::ConstParam) {
        let p = i;
        self.names.insert(p.ident.to_string());
        visit::visit_const_param(self, p);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// G5: cheat tokens

fn cheats(rel: &str, src: &str, r: &mut Report) {
    let stream = match syn::parse_str(src) {
        Ok(s) => s,
        Err(e) => {
            r.add("G5", rel, format!("cannot be tokenized: {e}"));
            return;
        }
    };
    let buf = TokenBuffer::new2(stream);
    let mut found = BTreeSet::new();
    idents_in(buf.begin(), &mut |i| {
        if CHEATS.contains(&i) {
            found.insert(i.to_owned());
        }
    });
    for c in found {
        r.add(
            "G5",
            rel,
            format!("cheat token `{c}` (only proofs/verus/trusted/ may weaken a proof, and it is listed in {REGISTRY})"),
        );
    }
}

/// Calls `f` on every identifier of the token stream at `start`, groups included.
fn idents_in(start: Cursor<'_>, f: &mut dyn FnMut(&str)) {
    let mut c = start;
    while !c.eof() {
        if let Some((inside, _, _, next)) = c.any_group() {
            idents_in(inside, f);
            c = next;
        } else if let Some((ident, next)) = c.ident() {
            f(&ident.to_string());
            c = next;
        } else if let Some((_, next)) = c.token_tree() {
            c = next;
        } else {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// G7: `reason = "verus: …"` only in mounted kernel files

fn g7(tree: &Tree, reg: &Registry, r: &mut Report) -> anyhow::Result<()> {
    for dir in ["crates", "fuzz"] {
        for f in tree.rust_files(dir)? {
            let Some(src) = tree.read(&f) else { continue };
            if !src.contains("verus:") {
                continue;
            }
            let mounted = reg
                .kernel_of_file(&f)
                .is_some_and(|k| k.status == Status::Verified);
            if mounted {
                continue; // the kernel walk checks placement
            }
            let Ok(file) = syn::parse_file(&src) else {
                continue; // not this guard's business
            };
            let mut v = VerusReasons::default();
            v.visit_file(&file);
            if v.count > 0 {
                r.add(
                    "G7",
                    &f,
                    format!("{} `reason = \"verus: …\"` expectation(s) outside a mounted kernel file: nothing proves them", v.count),
                );
            }
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct VerusReasons {
    count: usize,
}

impl<'ast> Visit<'ast> for VerusReasons {
    fn visit_attribute(&mut self, i: &'ast syn::Attribute) {
        let a = i;
        if (a.path().is_ident("expect") || a.path().is_ident("allow"))
            && let syn::Meta::List(l) = &a.meta
            && has_verus_reason(&TokenBuffer::new2(l.tokens.clone()))
        {
            self.count = self.count.saturating_add(1);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// G8: impls of kernel traits

fn g8(tree: &Tree, reg: &Registry, r: &mut Report) -> anyhow::Result<()> {
    // trait name → declaring kernel id
    let mut traits: BTreeMap<String, String> = BTreeMap::new();
    for k in &reg.kernels {
        let Some(file) = tree.read(&k.file).and_then(|s| syn::parse_file(&s).ok()) else {
            continue;
        };
        let mut t = Traits::default();
        t.visit_file(&file);
        for name in t.names {
            traits.insert(name, k.id.clone());
        }
    }
    let kernel_files: BTreeSet<&str> = reg.kernels.iter().map(|k| k.file.as_str()).collect();
    let mut found: BTreeSet<String> = BTreeSet::new();
    if !traits.is_empty() {
        for f in tree.rust_files("crates")? {
            if kernel_files.contains(f.as_str()) {
                continue;
            }
            let Some(src) = tree.read(&f) else { continue };
            if !traits.keys().any(|t| src.contains(t.as_str())) {
                continue;
            }
            let Ok(file) = syn::parse_file(&src) else {
                continue;
            };
            let mut v = Impls {
                traits: &traits,
                found: Vec::new(),
            };
            v.visit_file(&file);
            for (item, kernel) in v.found {
                let entry = reg.trusted.iter().find(|t| norm_item(&t.item) == item);
                match entry {
                    None => r.add(
                        "G8",
                        &f,
                        format!("`impl {item}` implements a trait of kernel {kernel} outside kernel files: add a [[trusted]] entry with its differential test"),
                    ),
                    Some(t) if t.kernel != kernel => r.add(
                        "G8",
                        REGISTRY,
                        format!("[[trusted]] {} names kernel {}, but the trait belongs to {kernel}", t.item, t.kernel),
                    ),
                    Some(_) => {}
                }
                found.insert(item);
            }
        }
    }
    for t in &reg.trusted {
        if !reg.kernels.iter().any(|k| k.id == t.kernel) {
            r.add(
                "G8",
                REGISTRY,
                format!("[[trusted]] {}: unknown kernel {}", t.item, t.kernel),
            );
        }
        if !found.contains(&norm_item(&t.item)) {
            r.add(
                "G8",
                REGISTRY,
                format!(
                    "[[trusted]] {}: no such impl of a kernel trait (stale entry)",
                    t.item
                ),
            );
        }
        match fingerprint::split_anchor(&t.test) {
            Ok((file, test)) => {
                let exists = tree
                    .read(file)
                    .and_then(|s| syn::parse_file(&s).ok())
                    .is_some_and(|f| !fns_named(&f, test).is_empty());
                if !exists {
                    r.add(
                        "G8",
                        REGISTRY,
                        format!("[[trusted]] {}: test {} does not exist", t.item, t.test),
                    );
                }
            }
            Err(e) => r.add("G8", REGISTRY, format!("[[trusted]] {}: {e:#}", t.item)),
        }
    }
    Ok(())
}

/// `Type : Trait` → `Type: Trait`.
fn norm_item(s: &str) -> String {
    match s.split_once(':') {
        Some((ty, tr)) => format!("{}: {}", ty.trim(), tr.trim()),
        None => s.trim().to_owned(),
    }
}

#[derive(Debug, Default)]
struct Traits {
    names: Vec<String>,
}

impl<'ast> Visit<'ast> for Traits {
    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        let t = i;
        self.names.push(t.ident.to_string());
        visit::visit_item_trait(self, t);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        let m = i;
        if !m.attrs.iter().any(is_test_cfg) {
            visit::visit_item_mod(self, m);
        }
    }
}

#[derive(Debug)]
struct Impls<'t> {
    traits: &'t BTreeMap<String, String>,
    /// (`Type: Trait`, kernel id)
    found: Vec<(String, String)>,
}

impl<'ast> Visit<'ast> for Impls<'_> {
    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if let Some((_, path, _)) = &i.trait_
            && let Some(last) = path.segments.last()
            && let Some(kernel) = self.traits.get(&last.ident.to_string())
        {
            let ty = fingerprint::type_name(&i.self_ty);
            self.found
                .push((format!("{ty}: {}", last.ident), kernel.clone()));
        }
        visit::visit_item_impl(self, i);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// G10: callers, model files and anchors

fn g10(tree: &Tree, reg: &Registry, r: &mut Report) {
    for k in &reg.kernels {
        let entries: BTreeSet<String> = tree
            .read(&k.file)
            .and_then(|s| syn::parse_file(&s).ok())
            .map_or_default(|f| entry_points(&f));
        for c in &k.callers {
            let (file, item) = match fingerprint::split_anchor(c) {
                Ok(x) => x,
                Err(e) => {
                    r.add("G10", REGISTRY, format!("{}: caller {e:#}", k.id));
                    continue;
                }
            };
            let Some(parsed) = tree.read(file).and_then(|s| syn::parse_file(&s).ok()) else {
                r.add(
                    "G10",
                    REGISTRY,
                    format!(
                        "{}: caller file {file} does not exist or does not parse",
                        k.id
                    ),
                );
                continue;
            };
            let name = item.rsplit("::").next().unwrap_or(item);
            let bodies = fns_named(&parsed, name);
            if bodies.is_empty() {
                r.add(
                    "G10",
                    file,
                    format!("caller `{item}` of kernel {} does not exist", k.id),
                );
                continue;
            }
            let calls = bodies.iter().any(|b| {
                let mut n = Names::default();
                n.visit_block(b);
                n.names.iter().any(|x| entries.contains(x))
            });
            if !calls {
                r.add(
                    "G10",
                    file,
                    format!(
                        "`{item}` does not call kernel {} (entry points: {})",
                        k.id,
                        entries.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                );
            }
        }
    }

    let lakefile = tree
        .read(&format!("{LEAN_DIR}/lakefile.toml"))
        .unwrap_or_default();
    for m in &reg.models {
        for (what, path) in [("lean", &m.lean), ("oracle_test", &m.oracle_test)] {
            if !tree.is_file(path) {
                r.add(
                    "G10",
                    REGISTRY,
                    format!("model {}: {what} file {path} does not exist", m.id),
                );
            }
        }
        if !lakefile.contains(&format!("name = \"{}\"", m.oracle)) {
            r.add(
                "G10",
                REGISTRY,
                format!(
                    "model {}: oracle `{}` is not a lean_exe of {LEAN_DIR}/lakefile.toml",
                    m.id, m.oracle
                ),
            );
        }
        if m.theorems.is_empty() {
            r.add(
                "G10",
                REGISTRY,
                format!("model {}: theorems must list at least one theorem", m.id),
            );
        }
        if m.anchors.is_empty() {
            r.add(
                "G10",
                REGISTRY,
                format!(
                    "model {}: at least one anchor pins the production items it models",
                    m.id
                ),
            );
        }
        for a in &m.anchors {
            match fingerprint::of_anchor(tree, &a.item) {
                Err(e) => r.add("G10", REGISTRY, format!("model {}: {e:#}", m.id)),
                Ok(fp) if fp != a.fingerprint => r.add(
                    "G10",
                    REGISTRY,
                    format!(
                        "model {}: model link stale: {} changed (fingerprint {fp}, registry {}): re-review the model against the code, then update the fingerprint",
                        m.id, a.item, a.fingerprint
                    ),
                ),
                Ok(_) => {}
            }
        }
    }
}

/// Non-private functions of a kernel file (its entry points).
fn entry_points(file: &syn::File) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in &file.items {
        match item {
            syn::Item::Fn(f) if !matches!(f.vis, syn::Visibility::Inherited) => {
                out.insert(f.sig.ident.to_string());
            }
            syn::Item::Impl(i) => {
                for ii in &i.items {
                    if let syn::ImplItem::Fn(f) = ii
                        && (i.trait_.is_some() || !matches!(f.vis, syn::Visibility::Inherited))
                    {
                        out.insert(f.sig.ident.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The bodies of every function named `name` (free, nested or in an impl).
fn fns_named<'f>(file: &'f syn::File, name: &str) -> Vec<&'f syn::Block> {
    struct Find<'f, 'n> {
        name: &'n str,
        out: Vec<&'f syn::Block>,
    }
    impl<'f> Visit<'f> for Find<'f, '_> {
        fn visit_item_fn(&mut self, i: &'f syn::ItemFn) {
            let f = i;
            if f.sig.ident == self.name {
                self.out.push(&f.block);
            }
            visit::visit_item_fn(self, f);
        }
        fn visit_impl_item_fn(&mut self, i: &'f syn::ImplItemFn) {
            let f = i;
            if f.sig.ident == self.name {
                self.out.push(&f.block);
            }
            visit::visit_impl_item_fn(self, f);
        }
    }
    let mut v = Find {
        name,
        out: Vec::new(),
    };
    v.visit_file(file);
    v.out
}

/// Last path segments and method names used in a body.
#[derive(Debug, Default)]
struct Names {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Names {
    fn visit_path(&mut self, i: &'ast syn::Path) {
        let p = i;
        if let Some(s) = p.segments.last() {
            self.names.insert(s.ident.to_string());
        }
        visit::visit_path(self, p);
    }

    fn visit_expr_method_call(&mut self, i: &'ast syn::ExprMethodCall) {
        let m = i;
        self.names.insert(m.method.to_string());
        visit::visit_expr_method_call(self, m);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The V2 manifest: every function syn finds in a kernel file

/// Every function of a kernel file as Verus names it below the mount: `f`, `inner::f`, `Type::method`,
/// `Trait::method`; `#[cfg(test)]` modules are skipped. The flag marks impl methods (Verus may name the impl
/// `impl&%N` when its self type is not a path).
pub(crate) fn kernel_fns(src: &str) -> anyhow::Result<Vec<(String, bool)>> {
    let file = syn::parse_file(src).context("parsing the kernel file")?;
    let mut out = Vec::new();
    fns_in(&file.items, "", &mut out);
    Ok(out)
}

fn fns_in(items: &[syn::Item], prefix: &str, out: &mut Vec<(String, bool)>) {
    for item in items {
        match item {
            syn::Item::Fn(f) => out.push((format!("{prefix}{}", f.sig.ident), false)),
            syn::Item::Impl(i) => {
                let ty = fingerprint::type_name(&i.self_ty);
                for ii in &i.items {
                    if let syn::ImplItem::Fn(f) = ii {
                        out.push((format!("{prefix}{ty}::{}", f.sig.ident), true));
                    }
                }
            }
            syn::Item::Trait(t) => {
                for ti in &t.items {
                    if let syn::TraitItem::Fn(f) = ti {
                        out.push((format!("{prefix}{}::{}", t.ident, f.sig.ident), false));
                    }
                }
            }
            syn::Item::Mod(m) if !m.attrs.iter().any(is_test_cfg) => {
                if let Some((_, inner)) = &m.content {
                    fns_in(inner, &format!("{prefix}{}::", m.ident), out);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Token helpers

/// Whether an identifier `word` occurs anywhere in a token stream (`TokenBuffer::new2(meta_list.tokens.clone())`).
fn has_ident(buf: &TokenBuffer, word: &str) -> bool {
    let mut hit = false;
    idents_in(buf.begin(), &mut |i| hit |= i == word);
    hit
}

/// `reason = "verus: …"` among the tokens of a lint attribute.
fn has_verus_reason(buf: &TokenBuffer) -> bool {
    let mut c = buf.begin();
    while !c.eof() {
        if let Some((ident, next)) = c.ident()
            && ident == "reason"
            && let Some((eq, after)) = next.punct()
            && eq.as_char() == '='
            && let Some((lit, _)) = after.literal()
        {
            let text = lit.to_string();
            let body = text
                .trim_start_matches('r')
                .trim_start_matches('#')
                .trim_start_matches('"');
            if body.starts_with("verus:") {
                return true;
            }
        }
        match c.token_tree() {
            Some((_, next)) => c = next,
            None => break,
        }
    }
    false
}

/// `a::b::c`.
fn path_str(p: &syn::Path) -> String {
    let lead = if p.leading_colon.is_some() { "::" } else { "" };
    let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
    format!("{lead}{}", segs.join("::"))
}

/// A short rendering of a meta item for messages.
fn meta_str(m: &syn::Meta) -> String {
    match m {
        syn::Meta::Path(p) => path_str(p),
        syn::Meta::List(l) => format!("{}({})", path_str(&l.path), l.tokens),
        syn::Meta::NameValue(nv) => format!("{} = …", path_str(&nv.path)),
    }
}

#[cfg(test)]
mod tests;
