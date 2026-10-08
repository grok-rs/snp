//! PyYAML `SafeLoader`/`SafeDumper` compatibility over libyaml-safer: arena composer, YAML 1.1 implicit resolver,
//! `SafeConstructor` to `cfgv::Value`, the C loader's exact error texts and a `CSafeDumper` emitter
//! (ARCHITECTURE §1.4.6, 01 §12, ADR-002).
//!
//! Owner: WP-14.

mod compose;
mod construct;
mod decode;
pub mod emit;
mod error;
mod limits;
mod resolve;

pub use compose::{Document, Node, NodeId, NodeKind, ScalarStyle, compose};
pub use construct::{construct, load};
pub use decode::decode_text;
pub use error::{Mark, YamlError};
pub use limits::{Limits, Schema};
pub use resolve::{ImplicitTag, resolve_plain};
