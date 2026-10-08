//! Conformance of snp-pyparsers with the Python code each validator ports, driven by the CPython vectors in
//! `tests/vectors/` (WBS WP-17 acceptance; regenerate with `cargo xtask gen-pch-data vectors`).
//!
//! Owner: WP-17.

#[cfg(test)]
mod support;

#[cfg(test)]
mod json;

#[cfg(test)]
mod difflib;

#[cfg(test)]
mod ini;
