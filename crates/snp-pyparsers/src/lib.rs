//! CPython-compatible validators for native hooks: json, YAML, tomllib, expat, difflib, INI (ARCHITECTURE §1.4.11,
//! 09 §9.1, §9.4–§9.5).
//!
//! Every parser here is a port of the Python code the corresponding pre-commit-hooks module runs (CPython's `json`
//! C scanner, ruamel.yaml ≥ 0.19's pure-Python loader, `tomllib`, expat as `xml.sax` drives it, `difflib`,
//! `configparser`) and reproduces its acceptance and message texts byte for byte, verified by the vectors under
//! `tests/vectors/` (generated from the CPython 3.12.3 and 3.13 oracles by `cargo xtask gen-pch-data vectors`). All
//! parsers are iterative (no recursion on input nesting), never panic and never allocate more than linearly in the
//! input; Python's own recursion limits are emulated as the crash path where the hooks hit them.
//!
//! The frozen API (`Result<_, String>`) renders an uncaught Python exception as its traceback's last line
//! (`Type: text`); the `*_ex`/`*_named` additions keep the difference between a reported error (the hook prints it
//! and continues) and a crash (exit 1, the rest of the partition is skipped, 09 §2.6) as [`CheckError`].
//!
//! Owner: WP-17.

pub mod difflib;
pub mod ini;
pub mod json;
pub mod toml;
pub mod xml;
pub mod yamlcheck;

mod error;
mod pysort;
mod pystr;

pub use error::CheckError;

use snp_pycompat as _;
