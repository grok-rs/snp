//! The only crate that touches the network: verified HTTPS downloads, safe archive extraction and the toolchain
//! catalogue (ARCHITECTURE §1.4.9, §6.3, ADR-034, ADR-077).
//!
//! Owner: WP-16.

pub mod archive;
mod error;
mod fetch;
pub mod tools;

pub use error::{Checksum, NetError};
pub use fetch::{Fetch, HttpFetch};
