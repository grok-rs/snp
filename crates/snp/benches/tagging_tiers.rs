//! Criterion micro-bench: tiered identify tagging (name / stat / content tiers) (ARCHITECTURE §12.3).
//!
//! Owner: WP-36. W0 stub: an empty benchmark until WP-36 fills it.

use criterion::{Criterion, criterion_group, criterion_main};
use snp_files as _;

fn bench(_c: &mut Criterion) {}

criterion_group!(benches, bench);
criterion_main!(benches);
