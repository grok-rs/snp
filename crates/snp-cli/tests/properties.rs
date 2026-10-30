//! Property tests of the CLI front end (12 §A.16): on random command lines built from the grammar of both specs
//! (option strings and their prefixes, `=` forms, clusters, `--`, negative numbers, empty strings, spaces, command
//! names and near misses, non-UTF-8 bytes) and random `COLUMNS`/`PRE_COMMIT_COLOR`, `snp_cli::parse` never panics,
//! is deterministic, and every outcome has argparse's shape: help/version text ends with one newline; a usage error
//! is a `usage: {prog}…` block followed by exactly one `{prog}…: error: …` line.
//!
//! Owner: WP-12.

#[cfg(test)]
mod properties {
    use std::ffi::OsString;

    use snp_cli::CliOutcome;
    use snp_core::brand::Brand;
    use snp_core::env::Env;

    /// xorshift64*: deterministic, dependency-free.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap()).unwrap()
        }

        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[self.below(items.len())]
        }
    }

    /// Every option string and command name of a spec tree.
    fn vocabulary(spec: &'static snp_argparse::ParserSpec, out: &mut Vec<&'static str>) {
        out.extend(["-h", "--help"]);
        for a in spec.actions {
            out.extend(a.opts.iter().copied());
            if let Some(c) = a.choices {
                out.extend(c.iter().copied());
            }
            if let snp_argparse::Kind::SubParsers(subs) = a.kind {
                for s in subs {
                    out.push(s.name);
                    vocabulary(s.spec, out);
                }
            }
        }
    }

    fn token(rng: &mut Rng, vocab: &[&'static str]) -> OsString {
        const EXTRA: &[&str] = &[
            "--", "-", "", " ", "a b", "-1", "-.5", "-1x", "--bogus", "-x", "---", "=", "--=x",
            "lint:js", "x.py", "auto", "never", "bogus", "ru", "commit", "push", "-av", "-vac",
            "-ft", "--co", "--c", "--s",
        ];
        let base = if rng.below(4) == 0 {
            rng.pick(EXTRA)
        } else {
            rng.pick(vocab)
        };
        let mut s = base.to_owned();
        match rng.below(8) {
            // A prefix (abbreviation or near miss).
            0 if s.len() > 2 => {
                let keep = 2 + rng.below(s.len() - 2);
                s = s.chars().take(keep).collect();
            }
            // `--opt=value` / attached short values.
            1 => {
                s.push_str(if rng.below(2) == 0 { "=" } else { "" });
                s.push_str(rng.pick(&["x", "", "never", "1", "-v"]));
            }
            _ => {}
        }
        let os = OsString::from(s);
        #[cfg(unix)]
        let os = if rng.below(30) == 0 {
            use std::os::unix::ffi::OsStringExt as _;
            let mut b = os.into_vec();
            b.push(0xff);
            OsString::from_vec(b)
        } else {
            os
        };
        os
    }

    fn check_shape(brand: Brand, out: &CliOutcome, argv: &[OsString]) {
        let prog = brand.prog();
        match out {
            CliOutcome::Stdout(s) => {
                assert!(s.ends_with('\n') && !s.ends_with("\n\n"), "{argv:?}: {s:?}");
            }
            CliOutcome::Usage(s) => {
                assert!(s.starts_with(&format!("usage: {prog}")), "{argv:?}: {s:?}");
                assert!(s.ends_with('\n'), "{argv:?}: {s:?}");
                let last = s.lines().last().unwrap();
                assert!(
                    last.starts_with(prog) && last.contains(": error: "),
                    "{argv:?}: {s:?}"
                );
                assert_eq!(s.matches(": error: ").count(), 1, "{argv:?}: {s:?}");
            }
            CliOutcome::Cli(_) => {}
            _ => panic!("{argv:?}: unexpected outcome {out:?}"),
        }
    }

    #[test]
    fn random_command_lines() {
        let mut rng = Rng(0x0ddc_0ffe_e123_4567);
        for brand in [Brand::PreCommit, Brand::Snp] {
            let spec = if brand == Brand::PreCommit {
                snp_cli::PRE_COMMIT_SPEC
            } else {
                snp_cli::SNP_SPEC
            };
            let mut vocab = Vec::new();
            vocabulary(spec, &mut vocab);
            for _ in 0..2500 {
                let n = rng.below(8);
                let argv: Vec<OsString> = (0..n).map(|_| token(&mut rng, &vocab)).collect();
                let mut env = vec![(
                    "COLUMNS",
                    rng.pick(&["80", "1", "10", "40", "200", "", "abc", " 100 ", "0"]),
                )];
                if rng.below(3) == 0 {
                    env.push((
                        "PRE_COMMIT_COLOR",
                        rng.pick(&["auto", "always", "never", "bogus", ""]),
                    ));
                }
                let env = Env::from_pairs(env);
                let tty = (rng.below(3) == 0).then(|| rng.below(300));
                let first = snp_cli::parse(brand, &argv, &env, tty);
                let second = snp_cli::parse(brand, &argv, &env, tty);
                assert_eq!(format!("{first:?}"), format!("{second:?}"), "{argv:?}");
                check_shape(brand, &first, &argv);
            }
        }
    }
}
