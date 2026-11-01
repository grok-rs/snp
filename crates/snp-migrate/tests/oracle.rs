//! The converted configs under the pre-commit 4.6.2 oracle (WP-34 acceptance): `validate-config` accepts every golden
//! (portable: silently; snp mode: only `Unexpected key(s)` warnings), and the `runnable` fixture's conversions run
//! every stage with exit 0 (portable: no warning at all).
//!
//! `tests/vectors/oracle.json` holds the oracle's answers, keyed by golden with its SHA-256, so CI checks them without
//! Python; `cargo test -p snp-migrate --test oracle -- --ignored` re-runs the oracle (target/oracle/pc462-py312) and
//! rewrites the vectors.
//!
//! Owner: WP-34.

#[cfg(test)]
mod oracle {
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    use sha2::{Digest as _, Sha256};
    use snp_testkit::{Bins, Oracle, TestEnv};

    fn manifest() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    fn vectors_path() -> PathBuf {
        manifest().join("tests/vectors/oracle.json")
    }

    fn goldens() -> Vec<PathBuf> {
        let mut v: Vec<_> = std::fs::read_dir(manifest().join("tests/goldens"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let n = p.file_name().unwrap().to_str().unwrap();
                p.extension().is_some_and(|e| e == "yaml") && !n.contains(".overlay.")
            })
            .collect();
        v.sort();
        v
    }

    fn sha(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        Sha256::digest(bytes)
            .iter()
            .fold(String::new(), |mut s, b| {
                write!(s, "{b:02x}").unwrap();
                s
            })
    }

    /// The pre-commit invocations of a runnable golden.
    const RUNS: &[&[&str]] = &[
        &["run", "--all-files"],
        &["run", "--hook-stage", "manual", "--all-files"],
        &[
            "run",
            "--hook-stage",
            "pre-push",
            "--all-files",
            "--remote-name",
            "origin",
            "--remote-url",
            "https://example.invalid/r.git",
            "--local-branch",
            "refs/heads/master",
            "--remote-branch",
            "refs/heads/master",
        ],
        &[
            "run",
            "--hook-stage",
            "commit-msg",
            "--commit-msg-filename",
            "msg.txt",
        ],
        &[
            "run",
            "--hook-stage",
            "post-checkout",
            "--from-ref",
            "HEAD",
            "--to-ref",
            "HEAD",
            "--checkout-type",
            "1",
        ],
    ];

    fn copy_tree(from: &Path, rel: &Path, out: &mut Vec<(String, Vec<u8>, bool)>) {
        for e in std::fs::read_dir(from.join(rel)).unwrap() {
            let e = e.unwrap();
            let r = rel.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_tree(from, &r, out);
            } else {
                let bytes = std::fs::read(e.path()).unwrap();
                #[cfg(unix)]
                let exec = {
                    use std::os::unix::fs::PermissionsExt as _;
                    e.metadata().unwrap().permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let exec = false;
                out.push((r.to_str().unwrap().replace('\\', "/"), bytes, exec));
            }
        }
    }

    fn output(o: &snp_testkit::Output) -> Value {
        json!({
            "exit": o.code,
            "stdout": String::from_utf8_lossy(&o.stdout),
            "stderr": String::from_utf8_lossy(&o.stderr),
        })
    }

    /// Asks the oracle about every golden (`None`: no oracle installed; anything else failing is a test failure).
    #[expect(
        clippy::unwrap_in_result,
        reason = "test helper: a failing fixture read is a test failure"
    )]
    fn ask_oracle() -> Option<Value> {
        let env = TestEnv::new(Bins {
            snp: PathBuf::from("snp"),
            probe: PathBuf::from("snp-probe"),
        });
        env.oracle(Oracle::Pc462Py312, env.root())?;
        let mut out = serde_json::Map::new();
        for (k, golden) in goldens().iter().enumerate() {
            let name = golden.file_name().unwrap().to_str().unwrap().to_owned();
            let text = std::fs::read(golden).unwrap();
            let case = name.split('.').next().unwrap().to_owned();
            let mut repo = env.git_repo(&format!("r{k}"));
            if case == "runnable" {
                let mut files = Vec::new();
                copy_tree(
                    &manifest().join("tests/fixtures").join(&case),
                    Path::new(""),
                    &mut files,
                );
                for (p, bytes, exec) in files {
                    repo = if exec {
                        repo.exec_file(&p, &bytes)
                    } else {
                        repo.file(&p, &bytes)
                    };
                }
                repo = repo.file("msg.txt", b"subject\n");
            }
            let dir = repo
                .file(".pre-commit-config.yaml", &text)
                .stage_all()
                .commit("init")
                .build();
            let validate = env
                .oracle(Oracle::Pc462Py312, &dir)?
                .args(["validate-config", ".pre-commit-config.yaml"])
                .output();
            let mut runs = Vec::new();
            if case == "runnable" {
                for args in RUNS {
                    let o = env.oracle(Oracle::Pc462Py312, &dir)?.args(*args).output();
                    runs.push(json!({"args": args, "result": output(&o)}));
                }
            }
            out.insert(
                name,
                json!({"sha256": sha(&text), "validate": output(&validate), "runs": runs}),
            );
        }
        Some(Value::Object(out))
    }

    /// Only `Unexpected key(s)` warnings (snp mode) or nothing (portable) besides hook status lines.
    fn check_lines(name: &str, what: &str, text: &str, portable: bool) {
        for line in text.lines() {
            if line.starts_with("[WARNING] Unexpected key(s) present") {
                assert!(!portable, "{name} {what}: {line}");
                continue;
            }
            assert!(
                !line.starts_with('[') && !line.contains("rror"),
                "{name} {what}: unexpected line {line:?}"
            );
        }
    }

    fn check(vectors: &Value) {
        let map = vectors.as_object().unwrap();
        let goldens = goldens();
        assert_eq!(
            map.len(),
            goldens.len(),
            "vectors and goldens differ: regenerate the vectors"
        );
        for golden in goldens {
            let name = golden.file_name().unwrap().to_str().unwrap();
            let v = &map[name];
            assert_eq!(
                v["sha256"],
                sha(&std::fs::read(&golden).unwrap()),
                "{name}: stale vector, re-run the oracle"
            );
            let portable = name.contains(".portable.");
            assert_eq!(v["validate"]["exit"], 0, "{name}: {}", v["validate"]);
            check_lines(
                name,
                "validate-config",
                v["validate"]["stdout"].as_str().unwrap(),
                portable,
            );
            assert_eq!(v["validate"]["stderr"], "", "{name}");
            if portable {
                assert_eq!(v["validate"]["stdout"], "", "{name}");
            }
            for run in v["runs"].as_array().unwrap() {
                let r = &run["result"];
                assert_eq!(r["exit"], 0, "{name} {}: {r}", run["args"]);
                check_lines(name, "run", r["stdout"].as_str().unwrap(), portable);
                assert!(
                    r["stdout"].as_str().unwrap().contains("Passed"),
                    "{name} {}: {r}",
                    run["args"]
                );
            }
            if name.starts_with("runnable.") {
                assert_eq!(v["runs"].as_array().unwrap().len(), RUNS.len(), "{name}");
            }
        }
    }

    #[test]
    fn oracle_vectors_accept_every_golden() {
        let text = std::fs::read_to_string(vectors_path()).unwrap();
        check(&serde_json::from_str(&text).unwrap());
    }

    #[test]
    #[ignore = "oracle"]
    fn oracle_regenerate_vectors() {
        let Some(v) = ask_oracle() else {
            panic!("the pc462-py312 oracle is not installed (cargo xtask oracle install)");
        };
        check(&v);
        std::fs::create_dir_all(vectors_path().parent().unwrap()).unwrap();
        std::fs::write(
            vectors_path(),
            serde_json::to_string_pretty(&v).unwrap() + "\n",
        )
        .unwrap();
    }
}
