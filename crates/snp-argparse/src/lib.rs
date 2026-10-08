//! CPython 3.13 `argparse`-compatible parser and `HelpFormatter` driven by static [`ParserSpec`] tables
//! (ARCHITECTURE §1.4.3, 12 §A.13).
//!
//! Owner: WP-12.

mod namespace;
mod parse;
mod spec;
pub mod textwrap;

pub use namespace::Namespace;
pub use parse::{columns, export_json, format_help, format_usage, parse, prefix_stable};
pub use spec::{
    ActionSpec, Conv, DefaultValue, Kind, Nargs, Outcome, ParseCfg, ParserSpec, SubCommand, Value,
};
