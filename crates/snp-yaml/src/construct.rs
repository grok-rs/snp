//! `SafeConstructor` + implicit resolver → `cfgv::Value`.
//!
//! Owner: WP-14.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-14")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-14")]

use crate::{Document, Limits, Schema, YamlError};

/// `SafeConstructor` + implicit resolver → `cfgv::Value` (date, datetime, big ints, bytes, set, omap/pairs as tuples,
/// merge keys, duplicate keys last-wins at the first position with Python key equality 1 == 1.0 == True).
pub fn construct(doc: &Document, schema: Schema, lim: &Limits) -> Result<cfgv::Value, YamlError> {
    todo!()
}

/// `yaml.load(text, Loader=CSafeLoader)`: `PyYaml11Safe`, one document.
pub fn load(text: &str, lim: &Limits) -> Result<cfgv::Value, YamlError> {
    todo!()
}
