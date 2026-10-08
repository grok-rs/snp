//! `argparse.Namespace`: dest → value in declaration order, plus the chosen sub-command.
//!
//! Owner: WP-12.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-12")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-12")]

use std::ffi::{OsStr, OsString};

/// The parse result (`argparse.Namespace`).
#[derive(Debug, Clone)]
pub struct Namespace {
    /// W0 placeholder; WP-12 stores dest → [`crate::Value`] in declaration order and the sub-command path.
    _priv: (),
}

impl Namespace {
    /// The chosen sub-command and its namespace (`args.command`).
    pub fn command(&self) -> Option<(&'static str, &Self)> {
        todo!()
    }

    /// A boolean dest (`store_true` / `store_false`).
    pub fn flag(&self, dest: &str) -> bool {
        todo!()
    }

    /// A single string dest; `None` when unset.
    pub fn value(&self, dest: &str) -> Option<&OsStr> {
        todo!()
    }

    /// A list dest; empty when unset.
    pub fn list(&self, dest: &str) -> &[OsString] {
        todo!()
    }

    /// An integer dest.
    pub fn int(&self, dest: &str) -> Option<i64> {
        todo!()
    }

    /// Whether the option appeared on the command line (not just defaulted).
    pub fn was_given(&self, dest: &str) -> bool {
        todo!()
    }
}
