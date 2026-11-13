//! Generic spec functions and lemmas shared by the kernel specs (ADR-095, docs/VERIFICATION.md §8.2).
//!
//! Only kernel-independent facts belong here; a kernel's own definitions live in `spec/<kernel>.rs`.
//! Add `bytes.rs` (byte predicates) and `arith.rs` (integer facts) here when a kernel first needs them.
pub mod seq;
