//! Resource limits and the resolver schema.
//!
//! Owner: WP-14.

/// Limits that turn hostile documents into YAML errors (never a stack overflow or OOM).
#[derive(Copy, Clone, Debug)]
pub struct Limits {
    /// Maximum node nesting depth (512).
    pub max_depth: u32,
    /// Maximum constructed nodes, aliases counted at every use (1 000 000; alias bombs).
    pub max_constructed_nodes: u32,
    /// Maximum input size in bytes (16 MiB).
    pub max_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 512,
            max_constructed_nodes: 1_000_000,
            max_bytes: 16 << 20,
        }
    }
}

/// Implicit-resolver table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Schema {
    /// PyYAML's YAML 1.1 `SafeLoader` resolver (pre-commit files).
    PyYaml11Safe,
    /// YAML 1.2 core schema (lefthook reader only).
    Yaml12Core,
}
