//! `check-yaml`: a ruamel.yaml ≥ 0.19-compatible checker (09 §4.C, §9.5; `check_yaml.py`).
//!
//! Owner: WP-17.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-17")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-17")]

/// `check_yaml.py` loader modes.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Mode {
    /// Default: `YAML(typ='safe')`, one document.
    Safe,
    /// `--allow-multiple-documents` / `-m`.
    AllowMultipleDocuments,
    /// `--unsafe`: syntax check only.
    Unsafe,
}

/// `check_yaml.py`: `Err` carries ruamel.yaml's message (≥ 0.19 message classes).
pub fn check(text: &str, m: Mode) -> Result<(), String> {
    todo!()
}
