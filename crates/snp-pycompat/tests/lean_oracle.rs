//! Lean oracle test of kernel K4 (ADR-095, docs/VERIFICATION.md §9.1 K4, S3): `shlex::split`, `shlex::quote` and
//! `shlex::join` agree with the `shlex-model` executable, which runs the Lean model whose theorems
//! `pySplit_eq_split` (the transliteration of CPython's `read_token` equals the one-pass state machine),
//! `split_quote` and `split_join` (round trips) are proved in `proofs/lean/SnpModels/Shlex.lean`.
//!
//! Inputs: every string over `{' ', '\t', '\'', '"', '\\', 'a'}` up to length 7 (335,923 strings), plus seeded
//! random strings over a wider alphabet (all four separators, `#`, NUL, non-ASCII), every string up to length 5
//! over a quoting alphabet, and seeded random argument lists for `join`.
//!
//! Skipped (with a note) unless `SNP_LEAN_ORACLES` names the directory of the built oracles
//! (`proofs/lean/.lake/build/bin` after `lake build` in `proofs/lean`); a skip fails under
//! `SNP_REQUIRE_LEAN_ORACLES=1`. The crate keeps zero dependencies, dev-dependencies included (ADR-056): the
//! client is the std-only `proofs/lean/client.rs`, included here at the crate root.

#[cfg(test)]
#[path = "../../../proofs/lean/client.rs"]
mod client;

#[cfg(test)]
mod lean_oracle {
    use std::time::Instant;

    use snp_pycompat::shlex;

    use crate::client::{Oracle, SplitMix64, hex, words};

    /// The bounded-exhaustive split alphabet of §9.1 K4 S3.
    const SPLIT_EXHAUSTIVE: [char; 6] = [' ', '\t', '\'', '"', '\\', 'a'];
    /// Seeded random split inputs: every class `split` distinguishes, plus characters it must treat as ordinary.
    const SPLIT_RANDOM: [char; 16] = [
        ' ', '\t', '\r', '\n', '\'', '"', '\\', 'a', 'b', '#', '$', '\0', 'é', '\u{85}', '\u{a0}',
        '😀',
    ];
    /// Safe and unsafe characters for `quote` (`_find_unsafe` is ASCII-only, so `é` is unsafe).
    const QUOTE_ALPHABET: [char; 8] = ['a', '_', '/', '\'', '"', ' ', '\\', 'é'];

    const SPLIT_SEED: u64 = 0x5eed_0004_0000_0001;
    const SPLIT_ROUNDS: usize = 20_000;
    const JOIN_SEED: u64 = 0x5eed_0004_0000_0002;
    const JOIN_ROUNDS: usize = 5_000;

    /// The oracle's reply framing: `OK <n> :<hex>…`.
    fn ok_tokens<S: AsRef<str>>(tokens: &[S]) -> String {
        let mut reply = format!("OK {}", tokens.len());
        for t in tokens {
            reply.push_str(" :");
            reply.push_str(&hex(t.as_ref().as_bytes()));
        }
        reply
    }

    /// What `shlex-model split` must answer for `s`.
    fn rust_split(s: &str) -> String {
        match shlex::split(s) {
            Ok(tokens) => ok_tokens(&tokens),
            Err(e) => format!("ERR {e}"),
        }
    }

    fn check_split(model: &mut Oracle, s: &str, context: &str) {
        let reply = model.ask_op("split", s.as_bytes()).unwrap();
        assert_eq!(
            reply,
            rust_split(s),
            "split({s:?}) differs from {} ({context})",
            model.name()
        );
    }

    fn check_quote(model: &mut Oracle, s: &str) {
        let reply = model.ask_op("quote", s.as_bytes()).unwrap();
        let quoted = shlex::quote(s);
        assert_eq!(
            reply,
            ok_tokens(&[&*quoted]),
            "quote({s:?}) differs from {}",
            model.name()
        );
        assert_eq!(
            &*shlex::quote_bytes(s.as_bytes()),
            quoted.as_bytes(),
            "quote_bytes({s:?})"
        );
    }

    fn check_join(model: &mut Oracle, args: &[String], seed: u64) {
        let mut request = format!("join {}", args.len());
        for a in args {
            request.push_str(" :");
            request.push_str(&hex(a.as_bytes()));
        }
        let reply = model.ask(&request).unwrap();
        let joined = shlex::join(args);
        assert_eq!(
            reply,
            ok_tokens(&[&joined]),
            "join({args:?}) differs from {} (seed {seed:#x})",
            model.name()
        );
    }

    fn report(what: &str, cases: usize, start: Instant) {
        eprintln!(
            "lean oracle: {cases} {what} cases agree in {:.2?}",
            start.elapsed()
        );
    }

    #[test]
    fn split_matches_the_model_exhaustively() {
        let Some(mut model) = Oracle::from_env("shlex-model") else {
            return;
        };
        let start = Instant::now();
        let mut cases = 0_usize;
        let mut s = String::new();
        words(&SPLIT_EXHAUSTIVE, 7, |w| {
            s.clear();
            s.extend(w);
            check_split(&mut model, &s, "exhaustive");
            cases += 1;
        });
        assert_eq!(cases, 335_923);
        report("exhaustive split", cases, start);
    }

    #[test]
    fn split_matches_the_model_on_random_inputs() {
        let Some(mut model) = Oracle::from_env("shlex-model") else {
            return;
        };
        let start = Instant::now();
        let mut rng = SplitMix64(SPLIT_SEED);
        for round in 0..SPLIT_ROUNDS {
            let s: String = rng.word(&SPLIT_RANDOM, 48).into_iter().collect();
            check_split(
                &mut model,
                &s,
                &format!("seed {SPLIT_SEED:#x}, round {round}"),
            );
        }
        report("random split", SPLIT_ROUNDS, start);
    }

    #[test]
    fn quote_matches_the_model() {
        let Some(mut model) = Oracle::from_env("shlex-model") else {
            return;
        };
        let start = Instant::now();
        let mut cases = 0_usize;
        let mut s = String::new();
        words(&QUOTE_ALPHABET, 5, |w| {
            s.clear();
            s.extend(w);
            check_quote(&mut model, &s);
            cases += 1;
        });
        report("exhaustive quote", cases, start);
    }

    #[test]
    fn join_matches_the_model_on_random_lists() {
        let Some(mut model) = Oracle::from_env("shlex-model") else {
            return;
        };
        let start = Instant::now();
        let mut rng = SplitMix64(JOIN_SEED);
        check_join(&mut model, &[], JOIN_SEED);
        check_join(&mut model, &[String::new()], JOIN_SEED);
        for _ in 0..JOIN_ROUNDS {
            let n = rng.below(5);
            let args: Vec<String> = (0..n)
                .map(|_| rng.word(&SPLIT_RANDOM, 8).into_iter().collect())
                .collect();
            check_join(&mut model, &args, JOIN_SEED);
            // The proved round trip, checked on the Rust side too.
            assert_eq!(
                shlex::split(&shlex::join(&args)).unwrap(),
                args,
                "split(join({args:?}))"
            );
        }
        report("random join", JOIN_ROUNDS + 2, start);
    }
}
