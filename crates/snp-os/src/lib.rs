//! Durable filesystem primitives, contained deletion, file locks, terminal and colour decisions, home and realpath,
//! the `DiffOracle` filesystem allow-list and its root-relative `lstat` (ARCHITECTURE §1.4.7).
//!
//! Owner: WP-15.

pub mod fs;
pub mod fsinfo;
pub mod home;
pub mod lock;
pub mod realpath;
pub mod rootstat;
mod same;
pub mod term;

pub use same::same_file;
