//! `Language` trait, registry, shared helpers, `CmdRunner` seam and the 21 language backends (ARCHITECTURE §1.4.10).
//!
//! Owner: WP-16.

pub mod common;
mod framework;
#[cfg(feature = "testing")]
pub mod harness;
pub mod langs;
