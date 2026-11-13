//! Specs and lemmas of the verified kernels (ADR-095). Kernel files import `crate::spec::<kernel>::*` under
//! `cfg(verus_keep_ghost)`; they contain no `verus!` block themselves (docs/VERIFICATION.md §2.3 rule K4).
//!
//! One module per kernel (the registry entry's `spec`); generic lemmas go into `lib`.
pub mod archive_entry;
pub mod eof;
pub mod lib;
pub mod xargs_partition;
