//! Lean oracle test of kernel K3 (ADR-095, docs/VERIFICATION.md §9.1 K3, bridge): `kernels::end_of_file_bytes`
//! agrees with the `eof-model` executable, which runs the Lean transliteration `pyFixFile` of pre-commit-hooks'
//! `end_of_file_fixer.fix_file`. Lean proves it equal to the spec `fix` with the return code (`pyFixFile_eq`) and
//! `fix` idempotent (`fix_idem`); Verus proves the Rust kernel equal to the same `fix` (proofs/verus/spec/eof.rs).
//! The oracle also self-checks the Lean model of snp's kernel (`kernel_apply`) and answers `MISMATCH` on a
//! disagreement.
//!
//! Inputs: every byte string over `{a, CR, LF}` up to length 10 (88,573 strings), plus seeded random strings over
//! a wider alphabet (space, NUL, `0xff`, `\x0b`, `\x0c`, which are not line endings) and seeded long line-ending
//! runs.
//!
//! Skipped (with a note) unless `SNP_LEAN_ORACLES` names the directory of the built oracles
//! (`proofs/lean/.lake/build/bin` after `lake build` in `proofs/lean`); a skip fails under
//! `SNP_REQUIRE_LEAN_ORACLES=1`. Std only: the client is `proofs/lean/client.rs`, included at the crate root.
//!
//! Owner: WP-38 (formal verification) with WP-26.

#[cfg(test)]
#[path = "../../../proofs/lean/client.rs"]
mod client;

#[cfg(test)]
mod lean_oracle {
    use std::time::Instant;

    use snp_hooks::pch::p0::kernels;

    use crate::client::{Oracle, SplitMix64, hex, words};

    /// The bounded-exhaustive alphabet of §9.1 K3.
    const EXHAUSTIVE: &[u8; 3] = b"a\r\n";
    /// Seeded random inputs: line endings weighted up, plus bytes that look like whitespace but are not EOL.
    const RANDOM: &[u8; 10] = b"a\r\n\r\n \t\x0b\x00\xff";

    const SEED: u64 = 0x5eed_0003_0000_0001;
    const ROUNDS: usize = 20_000;

    /// What `eof-model` must answer for `content`: `OK <return code> :<hex of the new bytes>`.
    fn rust_answer(content: &[u8]) -> String {
        match kernels::end_of_file_bytes(content) {
            None => format!("OK 0 :{}", hex(content)),
            Some(fixed) => format!("OK 1 :{}", hex(&fixed)),
        }
    }

    fn check(model: &mut Oracle, content: &[u8], context: &str) {
        let reply = model.ask_hex(content).unwrap();
        assert_eq!(
            reply,
            rust_answer(content),
            "end_of_file_bytes({:?}) differs from {} ({context})",
            String::from_utf8_lossy(content),
            model.name()
        );
    }

    #[test]
    fn end_of_file_bytes_matches_the_model_exhaustively() {
        let Some(mut model) = Oracle::from_env("eof-model") else {
            return;
        };
        let start = Instant::now();
        let mut count = 0_usize;
        words(EXHAUSTIVE, 10, |w| {
            check(&mut model, w, "exhaustive");
            count += 1;
        });
        assert_eq!(count, 88_573);
        eprintln!(
            "eof-model: {count} exhaustive inputs agree ({:?})",
            start.elapsed()
        );
    }

    #[test]
    fn end_of_file_bytes_matches_the_model_on_random_inputs() {
        let Some(mut model) = Oracle::from_env("eof-model") else {
            return;
        };
        let mut rng = SplitMix64(SEED);
        for round in 0..ROUNDS {
            let mut content = rng.word(RANDOM, 64);
            // Every fourth input ends in a long run of line endings, past the two bytes the kernel inspects.
            if round % 4 == 0 {
                let run = rng.word(&EXHAUSTIVE[1..], 300);
                content.extend_from_slice(&run);
            }
            check(
                &mut model,
                &content,
                &format!("seed {SEED:#x}, round {round}"),
            );
        }
        eprintln!("eof-model: {ROUNDS} random inputs agree (seed {SEED:#x})");
    }
}
