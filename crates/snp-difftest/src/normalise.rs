//! Normalisation tiers and the brand map (§13.2, §9.6).
//!
//! Owner: WP-35.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-35")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-35")]

/// How strictly two outputs are compared.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Tier {
    /// Exact.
    A,
    /// Semantic (crash-log body, tracebacks → last line, …).
    B,
    /// Ignored (durations, patch names, store roots, …).
    C,
}

/// `tests/differential/brand_map.yaml`; non-argparse texts only.
#[derive(Debug)]
pub struct BrandMap {
    /// W0 placeholder; WP-35 stores the substitution table.
    _priv: (),
}

/// Applies the tier's rules (and the brand map for snp-brand runs) to `bytes`.
pub fn normalise(tier: Tier, bytes: &[u8], map: &BrandMap) -> Vec<u8> {
    todo!()
}
