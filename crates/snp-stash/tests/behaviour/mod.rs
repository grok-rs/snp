//! Behaviour tests beyond the catalog rows.

mod guard;
mod hidden;
mod index_proof;
mod oracle;
mod recovery;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod rename;
mod stage_fixed;
mod tree_elision;
