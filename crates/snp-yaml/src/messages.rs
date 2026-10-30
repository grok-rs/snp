//! Every user-visible text of snp-yaml (ARCHITECTURE §1.3 "Messages"). None names the tool, so none takes a `Brand`.
//!
//! The PyYAML texts are verbatim from PyYAML 6.0.3 (`yaml/constructor.py`, `_yaml.pyx`) and CPython 3.12
//! (`int()`, `float()`, `str.encode('ascii')`, `binascii`, `datetime`); the `LIMIT_*` texts are snp's own
//! (D-SEC-11). Golden tests: `tests/messages.rs`.
//!
//! Owner: WP-14.

use std::fmt::Write as _;

use snp_pycompat::repr::repr_str;

/// The mark name of every loader error: PyYAML reads a `str`, so `CParser` names the stream `<unicode string>`.
pub(crate) const STREAM_NAME: &str = "<unicode string>";

// --- composer (`_yaml.pyx` `CParser`) ---------------------------------------------------------------------------

/// `get_single_node`: the context of a second document.
pub(crate) const SINGLE_DOCUMENT_CONTEXT: &str = "expected a single document in the stream";
/// `get_single_node`: the problem of a second document.
pub(crate) const SINGLE_DOCUMENT_PROBLEM: &str = "but found another document";
/// `_compose_node`: an alias to an anchor that was never defined (the C composer omits the name).
pub(crate) const UNDEFINED_ALIAS: &str = "found undefined alias";
/// `_compose_node`: the context of a redefined anchor.
pub(crate) const DUPLICATE_ANCHOR_CONTEXT: &str = "found duplicate anchor; first occurrence";
/// `_compose_node`: the problem of a redefined anchor.
pub(crate) const DUPLICATE_ANCHOR_PROBLEM: &str = "second occurrence";

// --- constructor (`yaml/constructor.py`) --------------------------------------------------------------------------

/// `construct_mapping` / `flatten_mapping` context.
pub(crate) const WHILE_CONSTRUCTING_MAPPING: &str = "while constructing a mapping";
/// `construct_mapping`: a list, dict or set used as a key.
pub(crate) const UNHASHABLE_KEY: &str = "found unhashable key";
/// `construct_yaml_omap` context.
pub(crate) const WHILE_CONSTRUCTING_OMAP: &str = "while constructing an ordered map";
/// `construct_yaml_pairs` context.
pub(crate) const WHILE_CONSTRUCTING_PAIRS: &str = "while constructing pairs";
/// `construct_object` on a node that is (transitively) its own element.
pub(crate) const UNCONSTRUCTABLE_RECURSIVE: &str = "found unconstructable recursive node";

/// `construct_undefined`: `could not determine a constructor for the tag %r`.
pub(crate) fn undefined_tag(tag: &str) -> String {
    format!(
        "could not determine a constructor for the tag {}",
        repr_str(tag)
    )
}

/// `construct_scalar` on a collection: `expected a scalar node, but found %s`.
pub(crate) fn expected_scalar(found: &str) -> String {
    format!("expected a scalar node, but found {found}")
}

/// `construct_sequence` on a scalar or mapping: `expected a sequence node, but found %s`.
pub(crate) fn expected_sequence_node(found: &str) -> String {
    format!("expected a sequence node, but found {found}")
}

/// `construct_mapping` on a scalar or sequence: `expected a mapping node, but found %s`.
pub(crate) fn expected_mapping_node(found: &str) -> String {
    format!("expected a mapping node, but found {found}")
}

/// `flatten_mapping`: a `<<` value that is a scalar.
pub(crate) fn merge_expected_mapping_or_list(found: &str) -> String {
    format!("expected a mapping or list of mappings for merging, but found {found}")
}

/// `flatten_mapping`: an item of a `<<` sequence that is not a mapping.
pub(crate) fn merge_expected_mapping(found: &str) -> String {
    format!("expected a mapping for merging, but found {found}")
}

/// `construct_yaml_omap` / `construct_yaml_pairs`: the node is not a sequence.
pub(crate) fn expected_sequence(found: &str) -> String {
    format!("expected a sequence, but found {found}")
}

/// `construct_yaml_omap` / `construct_yaml_pairs`: an item is not a mapping.
pub(crate) fn expected_single_mapping(found: &str) -> String {
    format!("expected a mapping of length 1, but found {found}")
}

/// `construct_yaml_omap` / `construct_yaml_pairs`: an item mapping with `n != 1` pairs.
pub(crate) fn expected_single_item(n: usize) -> String {
    format!("expected a single mapping item, but found {n} items")
}

/// `construct_yaml_binary`: the scalar is not ASCII (`str(UnicodeEncodeError)` of the `ascii` codec).
pub(crate) fn binary_not_ascii(exc: &str) -> String {
    format!("failed to convert base64 data into ascii: {exc}")
}

/// `construct_yaml_binary`: `binascii.Error` from `base64.decodebytes`.
pub(crate) fn binary_bad_base64(exc: &str) -> String {
    format!("failed to decode base64 data: {exc}")
}

// --- Python exceptions that escape the constructor (cfgv reports `str(exc)` as the leaf) --------------------------

/// `IndexError` of `value[0]` on an empty scalar (`!!int ''`, `!!float ''`).
pub(crate) const STRING_INDEX_OUT_OF_RANGE: &str = "string index out of range";
/// `AttributeError` of `construct_yaml_timestamp` when the regex does not match.
pub(crate) const TIMESTAMP_NO_MATCH: &str = "'NoneType' object has no attribute 'groupdict'";
/// `TypeError` of `re.match(…, node.value)` when a `!!timestamp` node is a mapping.
pub(crate) const TIMESTAMP_NOT_STRING: &str = "expected string or bytes-like object, got 'list'";
/// `OverflowError` of `float * int` when a sexagesimal float has more than 174 parts.
pub(crate) const INT_TOO_LARGE_FOR_FLOAT: &str = "int too large to convert to float";
/// `binascii.Error`: padding missing at the end of the data.
pub(crate) const INCORRECT_PADDING: &str = "Incorrect padding";

/// `ValueError` of `int(s, base)`: `invalid literal for int() with base %d: %.200R`.
pub(crate) fn invalid_int_literal(base: u32, literal: &str) -> String {
    let r: String = repr_str(literal).chars().take(200).collect();
    format!("invalid literal for int() with base {base}: {r}")
}

/// `ValueError` of `float(s)`: `could not convert string to float: %R`.
pub(crate) fn invalid_float_literal(literal: &str) -> String {
    format!("could not convert string to float: {}", repr_str(literal))
}

/// `binascii.Error` for one data character too many.
pub(crate) fn base64_one_extra(data_chars: usize) -> String {
    format!(
        "Invalid base64-encoded string: number of data characters ({data_chars}) cannot be 1 more than a multiple of 4"
    )
}

/// `str(UnicodeEncodeError)` of the `ascii` codec over `chars[start..end]` (character positions).
pub(crate) fn ascii_encode_error(first_bad: char, start: usize, end: usize) -> String {
    let mut out = String::from("'ascii' codec can't encode ");
    if end.saturating_sub(start) <= 1 {
        let cp = u32::from(first_bad);
        out.push_str("character '");
        if cp <= 0xff {
            write!(out, "\\x{cp:02x}").unwrap_or_default();
        } else if cp <= 0xffff {
            write!(out, "\\u{cp:04x}").unwrap_or_default();
        } else {
            write!(out, "\\U{cp:08x}").unwrap_or_default();
        }
        write!(out, "' in position {start}").unwrap_or_default();
    } else {
        write!(
            out,
            "characters in position {start}-{}",
            end.saturating_sub(1)
        )
        .unwrap_or_default();
    }
    out.push_str(": ordinal not in range(128)");
    out
}

// --- snp's resource limits (D-SEC-11) -----------------------------------------------------------------------------

/// The input is larger than `Limits::max_bytes`.
pub(crate) fn limit_bytes(len: usize, max: usize) -> String {
    format!("the input is {len} bytes long, more than the limit of {max} bytes")
}

/// A collection nested deeper than `Limits::max_depth` (structurally or through aliases and merges).
pub(crate) fn limit_depth(max: u32) -> String {
    format!("found a node nested deeper than the limit of {max} levels")
}

/// More nodes than `Limits::max_constructed_nodes` (aliases counted at every use, merged pairs counted).
pub(crate) fn limit_nodes(max: u32) -> String {
    format!("found more than the limit of {max} nodes (aliases count at every use)")
}

/// An internal inconsistency: a hand-built [`crate::Document`] that refers to a missing node, or an event
/// sequence the parser should never produce.
pub(crate) const INVALID_DOCUMENT: &str = "found an invalid node reference in the document";
