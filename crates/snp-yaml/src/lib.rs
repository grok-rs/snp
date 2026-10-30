//! PyYAML `SafeLoader`/`SafeDumper` compatibility: libyaml's scanner and parser, an arena composer, the YAML 1.1
//! implicit resolver, `SafeConstructor` to `cfgv::Value`, the C loader's exact error texts and a `CSafeDumper`
//! emitter (ARCHITECTURE §1.4.6, 01 §12, ADR-002).
//!
//! pre-commit reads YAML with `yaml.load(text, Loader=CSafeLoader)` — libyaml 0.2.5's scanner and parser, PyYAML's
//! Python resolver and constructor — and writes it with `CSafeDumper`. This crate reproduces both, byte for byte:
//!
//! * [`decode_text`] reads a file the way cfgv does (strict UTF-8, universal newlines);
//! * [`compose`] / [`compose_all`] build the node arena (an alias is its anchor's node id, never a copy), with
//!   byte spans and C-loader marks for migrate-config;
//! * [`construct`] turns a document into the `cfgv::Value` PyYAML would return, in PyYAML's construction order,
//!   and [`load`] does both;
//! * [`emit::dump`] writes `yaml.dump` bytes, [`emit::redump`] is `yaml_dump(yaml_load(text))` with PyYAML's
//!   `&id001` anchors, and [`emit::scalar_for_rev`] quotes an autoupdate rev;
//! * every error's `Display` is `str(exc)` of the exception PyYAML raises, and [`YamlError::py_type`] its class.
//!
//! The scanner and parser are snp's panic-free port of libyaml 0.2.5 (the fork of libyaml-safer 0.3.0 that ADR-002
//! provides for: libyaml-safer panics on two inputs and differs from C libyaml on four others; see the `libyaml`
//! module). Everything is iterative, so nesting depth costs heap, never stack. Hostile input — alias bombs, merge
//! bombs, deep nesting, huge files, self-referencing values — is bounded by [`Limits`] and reported as a YAML error
//! (D-SEC-11). Parity is checked against committed oracle vectors (`tests/vectors`, `cargo xtask yaml-vectors`).
//!
//! Owner: WP-14.

mod base64;
mod bignum;
mod compose;
mod construct;
mod decode;
pub mod emit;
mod emitter;
mod error;
mod events;
mod libyaml;
mod limits;
mod messages;
mod pynum;
mod resolve;

pub use compose::{Document, Node, NodeId, NodeKind, ScalarStyle, compose, compose_all};
pub use construct::{construct, load};
pub use decode::decode_text;
pub use error::{Mark, YamlError};
pub use limits::{Limits, Schema};
pub use resolve::{ImplicitTag, resolve_plain};
