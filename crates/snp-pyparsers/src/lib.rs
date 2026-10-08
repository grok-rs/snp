//! CPython-compatible validators for native hooks: json, YAML, tomllib, expat, difflib, INI (ARCHITECTURE §1.4.11,
//! 09 §9.1, §9.4–§9.5).
//!
//! Owner: WP-17.

pub mod difflib;
pub mod ini;
pub mod json;
pub mod toml;
pub mod xml;
pub mod yamlcheck;

use snp_core as _;
use snp_pycompat as _;
