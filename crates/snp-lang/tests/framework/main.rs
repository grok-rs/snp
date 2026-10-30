//! snp-lang framework tests (WBS WP-16): the catalog rows T10-XRG-20…22 and T10-UTL-02, `check_support` texts in
//! both brands, `ToolProbe` memoisation and persistence, the install state, the error envelope, the real and the shim
//! runners, the local template, `rmtree`. The modules are declared `#[cfg(test)]` so their `#[test]` functions and
//! helpers count as test code.
//!
//! Owner: WP-16.

#[cfg(test)]
mod catalog;
#[cfg(test)]
mod errors;
#[cfg(test)]
mod harness;
#[cfg(test)]
mod probe;
#[cfg(test)]
mod rmtree;
#[cfg(test)]
mod runner;
#[cfg(test)]
mod state;
#[cfg(test)]
mod support;
#[cfg(test)]
mod template;
