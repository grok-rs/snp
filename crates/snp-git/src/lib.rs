//! Every git invocation with pre-commit's exact argv, the `GitBackend` seam and owned repositories (ARCHITECTURE §1.4.13).
//!
//! Owner: WP-18.

pub mod hookscripts;
pub mod pch_facts;
#[cfg(feature = "testing")]
pub mod testing;
