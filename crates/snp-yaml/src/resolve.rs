//! PyYAML's implicit resolver table (`resolver.py`) and the YAML 1.2 core switch.
//!
//! Owner: WP-14.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-14")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-14")]

use crate::Schema;

/// The implicit tag of a plain scalar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImplicitTag {
    /// `tag:yaml.org,2002:null`.
    Null,
    /// `tag:yaml.org,2002:bool`.
    Bool,
    /// `tag:yaml.org,2002:int`.
    Int,
    /// `tag:yaml.org,2002:float`.
    Float,
    /// `tag:yaml.org,2002:timestamp`.
    Timestamp,
    /// `tag:yaml.org,2002:merge`.
    Merge,
    /// `tag:yaml.org,2002:value`.
    Value,
    /// `tag:yaml.org,2002:str`.
    Str,
}

/// `Resolver.resolve(ScalarNode, s, (True, False))`: bool/int/float/null/timestamp/merge/value/str.
pub fn resolve_plain(s: &str, schema: Schema) -> ImplicitTag {
    todo!()
}
