//! CPython-compatible validators for native hooks: json, YAML, tomllib, expat, difflib, INI (ARCHITECTURE §1.4.11).
//!
//! Owner: WP-17.

pub mod difflib;
pub mod ini;
pub mod json;
pub mod toml;
pub mod xml;
pub mod yamlcheck;
