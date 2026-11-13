//! Fixture root for the guard unit tests.
#![cfg_attr(verus_keep_ghost, feature(proc_macro_hygiene))]
#![forbid(unsafe_code)]

#[allow(unused_imports)]
use vstd::prelude::*;

pub mod spec;

/// K9: demo kernel.
#[verus_verify]
#[path = "../../crates/demo/src/kern.rs"]
pub mod demo_kern;
