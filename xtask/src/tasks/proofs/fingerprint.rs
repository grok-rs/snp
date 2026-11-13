//! Anchor fingerprints (G10, docs/VERIFICATION.md §3.1): `fnv1a64` over the normalised tokens of an item.
//!
//! A Lean model is linked to production code by a test, never by a proof, so each `[[model]]` pins the items it
//! models: when one of them changes, the fingerprint no longer matches and `proofs check` says "model link stale".
//!
//! Normalisation: the item's tokens as syn lexes them (comments are gone), without its `#[doc]` attributes (doc
//! comments), each token rendered alone (identifiers and literals as written, punctuation one character at a time,
//! groups as their delimiters around their contents, a trailing comma before a closing delimiter dropped) and joined
//! by single spaces. Formatting and documentation edits
//! therefore keep the fingerprint; any change to the code, its attributes or its signature changes it.
//!
//! An anchor names `path/to/file.rs::item`, where `item` is a top-level item (`split`, `St`), an item of an inline
//! module (`inner::item`) or a method of an `impl` block (`Type::method`, for inherent and trait impls alike). Several
//! items with the same name (an `impl` method in two impls, a cfg-split pair) are fingerprinted together, in file
//! order.

use anyhow::{Context as _, bail};
use syn::buffer::Cursor;
use syn::parse::ParseStream;

use super::Tree;

/// One named item of a file and its normalised tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece {
    /// `name`, `module::name` or `Type::method`.
    pub(crate) name: String,
    /// The normalised token text.
    pub(crate) tokens: String,
}

/// The 64-bit FNV-1a hash of `bytes`.
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes
        .iter()
        .fold(OFFSET, |h, &b| (h ^ u64::from(b)).wrapping_mul(PRIME))
}

/// The fingerprint of anchor `anchor` (`file.rs::item`) in `tree`, as 16 lowercase hex digits.
pub(crate) fn of_anchor(tree: &Tree, anchor: &str) -> anyhow::Result<String> {
    let (file, item) = split_anchor(anchor)?;
    let src = tree
        .read(file)
        .with_context(|| format!("anchor {anchor}: {file} does not exist"))?;
    of_item(&src, item).with_context(|| format!("anchor {anchor}"))
}

/// `file.rs::item` → (`file.rs`, `item`).
pub(crate) fn split_anchor(anchor: &str) -> anyhow::Result<(&str, &str)> {
    match anchor.split_once(".rs::") {
        Some((file, item)) if !file.is_empty() && !item.is_empty() => {
            let file_end = file.len().saturating_add(3);
            let file = anchor
                .get(..file_end)
                .context("anchor path is not on a character boundary")?;
            Ok((file, item))
        }
        _ => bail!("an anchor is `path/to/file.rs::item`, not {anchor:?}"),
    }
}

/// The fingerprint of `item` in the Rust source `src`.
pub(crate) fn of_item(src: &str, item: &str) -> anyhow::Result<String> {
    let pieces = pieces(src)?;
    let matching: Vec<&str> = pieces
        .iter()
        .filter(|p| p.name == item)
        .map(|p| p.tokens.as_str())
        .collect();
    if matching.is_empty() {
        bail!("no item named `{item}`");
    }
    Ok(format!("{:016x}", fnv1a64(matching.join("\n").as_bytes())))
}

/// Every named item of `src` (top level, inline modules, `impl` methods) with its normalised tokens.
pub(crate) fn pieces(src: &str) -> anyhow::Result<Vec<Piece>> {
    let parser = |input: ParseStream<'_>| -> syn::Result<Vec<Piece>> {
        let mut out = Vec::new();
        input.call(syn::Attribute::parse_inner)?;
        items(input, "", &mut out)?;
        Ok(out)
    };
    syn::parse::Parser::parse_str(parser, src).context("parsing Rust source")
}

/// Items of one module body (`prefix` is `""` or `mod::`).
fn items(input: ParseStream<'_>, prefix: &str, out: &mut Vec<Piece>) -> syn::Result<()> {
    while !input.is_empty() {
        let start = input.cursor();
        let item: syn::Item = input.fork().parse()?;
        match &item {
            syn::Item::Impl(imp) => {
                let ty = type_name(&imp.self_ty);
                input.call(syn::Attribute::parse_outer)?;
                skip_to_brace(input)?;
                let content;
                syn::braced!(content in input);
                content.call(syn::Attribute::parse_inner)?;
                while !content.is_empty() {
                    let s = content.cursor();
                    let ii: syn::ImplItem = content.parse()?;
                    let e = content.cursor();
                    if let Some(name) = impl_item_name(&ii) {
                        out.push(Piece {
                            name: format!("{prefix}{ty}::{name}"),
                            tokens: render(s, e),
                        });
                    }
                }
            }
            syn::Item::Mod(m) if m.content.is_some() => {
                input.call(syn::Attribute::parse_outer)?;
                skip_to_brace(input)?;
                let content;
                syn::braced!(content in input);
                content.call(syn::Attribute::parse_inner)?;
                items(&content, &format!("{prefix}{}::", m.ident), out)?;
            }
            _ => {
                let _item: syn::Item = input.parse()?;
            }
        }
        let end = input.cursor();
        if let Some(name) = item_name(&item) {
            out.push(Piece {
                name: format!("{prefix}{name}"),
                tokens: render(start, end),
            });
        }
    }
    Ok(())
}

/// Advances `input` to the next top-level `{ … }` group (an `impl` or `mod` header).
fn skip_to_brace(input: ParseStream<'_>) -> syn::Result<()> {
    while !input.peek(syn::token::Brace) {
        input.step(|c| {
            c.token_tree()
                .map(|(_, next)| ((), next))
                .ok_or_else(|| c.error("expected a `{ … }` body"))
        })?;
    }
    Ok(())
}

fn item_name(item: &syn::Item) -> Option<String> {
    let ident = match item {
        syn::Item::Const(i) => &i.ident,
        syn::Item::Enum(i) => &i.ident,
        syn::Item::Fn(i) => &i.sig.ident,
        syn::Item::Mod(i) => &i.ident,
        syn::Item::Static(i) => &i.ident,
        syn::Item::Struct(i) => &i.ident,
        syn::Item::Trait(i) => &i.ident,
        syn::Item::TraitAlias(i) => &i.ident,
        syn::Item::Type(i) => &i.ident,
        syn::Item::Union(i) => &i.ident,
        syn::Item::Macro(i) => i.ident.as_ref()?,
        _ => return None,
    };
    Some(ident.to_string())
}

fn impl_item_name(item: &syn::ImplItem) -> Option<String> {
    match item {
        syn::ImplItem::Const(i) => Some(i.ident.to_string()),
        syn::ImplItem::Fn(i) => Some(i.sig.ident.to_string()),
        syn::ImplItem::Type(i) => Some(i.ident.to_string()),
        _ => None,
    }
}

/// The last path segment of an `impl`'s self type (`_` for other types), as Verus names impl methods.
pub(crate) fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map_or_else(|| "_".to_owned(), |s| s.ident.to_string()),
        _ => "_".to_owned(),
    }
}

/// The normalised tokens from `start` up to (excluding) `end`, which lie at the same nesting level.
fn render(start: Cursor<'_>, end: Cursor<'_>) -> String {
    let mut out = Vec::new();
    render_into(start, Some(end), &mut out);
    out.join(" ")
}

fn render_into(start: Cursor<'_>, end: Option<Cursor<'_>>, out: &mut Vec<String>) {
    let mut c = start;
    while !c.eof() && Some(c) != end {
        if let Some(next) = skip_doc_attr(c) {
            c = next;
        } else if let Some((inside, delim, _, next)) = c.any_group() {
            let (open, close) = match format!("{delim:?}").as_str() {
                "Parenthesis" => ("(", ")"),
                "Brace" => ("{", "}"),
                "Bracket" => ("[", "]"),
                _ => ("", ""),
            };
            if !open.is_empty() {
                out.push(open.to_owned());
            }
            render_into(inside, None, out);
            // rustfmt adds or drops a trailing comma when it re-wraps a list: not a code change.
            if out.last().is_some_and(|t| t == ",") {
                out.pop();
            }
            if !close.is_empty() {
                out.push(close.to_owned());
            }
            c = next;
        } else if let Some((ident, next)) = c.ident() {
            out.push(ident.to_string());
            c = next;
        } else if let Some((lt, next)) = c.lifetime() {
            out.push(format!("'{}", lt.ident));
            c = next;
        } else if let Some((p, next)) = c.punct() {
            out.push(p.as_char().to_string());
            c = next;
        } else if let Some((lit, next)) = c.literal() {
            out.push(lit.to_string());
            c = next;
        } else if let Some((_, next)) = c.token_tree() {
            c = next;
        } else {
            break;
        }
    }
}

/// When `c` starts a `#[doc …]` or `#![doc …]` attribute, the cursor after it.
fn skip_doc_attr(c: Cursor<'_>) -> Option<Cursor<'_>> {
    let (p, mut next) = c.punct()?;
    if p.as_char() != '#' {
        return None;
    }
    if let Some((bang, after)) = next.punct()
        && bang.as_char() == '!'
    {
        next = after;
    }
    let (inside, delim, _, after) = next.any_group()?;
    if format!("{delim:?}") != "Bracket" {
        return None;
    }
    let (ident, _) = inside.ident()?;
    (ident == "doc").then_some(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_the_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn formatting_comments_and_docs_keep_the_fingerprint() {
        let one = "/// Splits.\npub fn split(s: &str) -> Vec<String> { s.split(' ').map(str::to_owned).collect() }\n";
        let wrapped = "// a comment\n/// Splits a string\n/// on spaces.\npub fn split(\n    s: &str,\n) -> Vec<String> {\n    // here\n    s.split(' ')\n        .map(str::to_owned)\n        .collect()\n}\n";
        assert_eq!(
            of_item(one, "split").unwrap(),
            of_item(wrapped, "split").unwrap()
        );
        let tab = one.replace("' '", "'\\t'");
        assert_ne!(
            of_item(one, "split").unwrap(),
            of_item(&tab, "split").unwrap()
        );
        let vis = one.replace("pub fn", "pub(crate) fn");
        assert_ne!(
            of_item(one, "split").unwrap(),
            of_item(&vis, "split").unwrap()
        );
        let attr = format!("#[inline]\n{one}");
        assert_ne!(
            of_item(one, "split").unwrap(),
            of_item(&attr, "split").unwrap()
        );
    }

    #[test]
    fn other_items_do_not_affect_the_fingerprint() {
        let a = "fn split() -> u8 { 1 }\n";
        let b = "const X: u8 = 2;\nfn split() -> u8 { 1 }\nfn other() {}\n";
        assert_eq!(of_item(a, "split").unwrap(), of_item(b, "split").unwrap());
        assert!(of_item(a, "missing").is_err());
    }

    #[test]
    fn methods_and_module_items_are_addressable() {
        let src = "
            mod inner { pub(crate) fn f() -> u8 { 1 } }
            struct St;
            impl St { fn step(&self) -> u8 { 2 } const K: u8 = 3; }
            impl Clone for St { fn clone(&self) -> Self { St } }
        ";
        let names: Vec<String> = pieces(src).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["inner::f", "inner", "St", "St::step", "St::K", "St::clone"]
        );
        let v1 = of_item(src, "St::step").unwrap();
        let v2 = of_item(&src.replace("2 }", "4 }"), "St::step").unwrap();
        assert_ne!(v1, v2);
        assert_eq!(v1.len(), 16);
    }

    #[test]
    fn anchors_split_at_the_file_name() {
        assert_eq!(
            split_anchor("crates/a/src/shlex.rs::split").unwrap(),
            ("crates/a/src/shlex.rs", "split")
        );
        assert_eq!(
            split_anchor("crates/a/src/k.rs::St::step").unwrap(),
            ("crates/a/src/k.rs", "St::step")
        );
        assert!(split_anchor("crates/a/src/shlex::split").is_err());
        assert!(split_anchor("x.rs::").is_err());
    }
}
