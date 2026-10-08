//! PyYAML `CSafeDumper` emitter (`yaml.dump` byte parity, 07 §3.1, 03 §11.3).
//!
//! Owner: WP-14.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-14")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-14")]

/// `yaml.dump(o, Dumper=CSafeDumper, default_flow_style=False, indent=4, sort_keys=False)` — byte exact.
pub fn dump(v: &cfgv::Value) -> String {
    todo!()
}

/// `default_style` of the autoupdate rev re-dump.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Quote {
    /// `None` (plain when possible).
    Plain,
    /// `"'"`.
    Single,
    /// `'"'`.
    Double,
}

/// autoupdate.py:146-149: `yaml_dump({'rev': rev}, default_style=q).split(':', 1)[1].strip()`.
pub fn scalar_for_rev(rev: &str, q: Quote) -> String {
    todo!()
}
