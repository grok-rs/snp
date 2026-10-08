//! snp-net integration tests (WBS WP-16 acceptance): verified downloads against local HTTPS/HTTP servers and a
//! CONNECT proxy, safe archive extraction against hostile archives, and the toolchain catalogue for every release
//! target. The modules are declared `#[cfg(test)]` so their `#[test]` functions and helpers count as test code.
//!
//! Owner: WP-16.

#[cfg(test)]
mod archive;
#[cfg(test)]
mod fetch;
#[cfg(test)]
mod support;
#[cfg(test)]
mod tools;
