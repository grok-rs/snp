//! The check-only loads (`check_config`, `check_manifest`; non-strict `validate-config` / `validate-manifest`)
//! against the full loads: for the pre-commit oracle corpus and for generated configs and manifests (valid and
//! broken), the check fails exactly when the load fails, with the same rendered error, the same `[WARNING]`
//! messages in the same order and the same regex `FutureWarning` lines; for a config that loads, applying the
//! schema defaults to the checked value gives the load's `Config::raw`.
//!
//! Owner: WP-21.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod check_only {
    use std::ffi::OsStr;
    use std::fmt::Write as _;

    use proptest::prelude::*;
    use proptest::strategy::ValueTree as _;
    use serde_json::Value as Json;
    use snp_config::validate::{self, FileKind};
    use snp_config::{ConfigError, ConfigSource, ExtMode, schema};
    use snp_core::brand::Brand;

    use crate::support::{BRANDS, Loaded, file_in_tempdir, vectors, with_opts};

    const MODES: [ExtMode; 2] = [ExtMode::Enabled, ExtMode::Strict];

    /// The observable outcome of one load: warnings, diagnostics, and the error (text and manifest flag).
    type Seen = (Vec<String>, Vec<String>, Option<(String, bool)>);

    fn seen<T>(l: &Loaded<T>) -> Seen {
        (
            l.warnings.clone(),
            l.diag.clone(),
            l.result
                .as_ref()
                .err()
                .map(|e: &ConfigError| (e.rendered.clone(), e.manifest)),
        )
    }

    /// Compares the check-only and the full load of `bytes` (`None`: missing file) as `file`; `Err` describes
    /// the first difference.
    fn compare(kind: &str, file: &str, bytes: Option<&[u8]>) -> Result<(), String> {
        for brand in BRANDS {
            for ext in MODES {
                let (dir, path) = file_in_tempdir(file, bytes);
                let src = ConfigSource {
                    path: path.clone(),
                    display: file.into(),
                    project_root: dir.path().to_path_buf(),
                };
                let tag = format!("[{brand:?}, {ext:?}]");
                if kind == "config" {
                    let full = with_opts(brand, ext, |o| snp_config::load_config(&src, o));
                    let check = with_opts(brand, ext, |o| snp_config::check_config(&src, o));
                    let via_validate = with_opts(brand, ext, |o| {
                        validate::validate(FileKind::Config, path.as_os_str(), o, false)
                    });
                    if seen(&full) != seen(&check) {
                        return Err(format!(
                            "{tag} config: load {:?} vs check {:?}",
                            seen(&full),
                            seen(&check)
                        ));
                    }
                    let validate_seen = seen(&via_validate);
                    let check_seen = seen(&check);
                    // `validate` displays the path as given (an absolute temp path here): compare the outcome
                    // kind and the messages, not the `File …` line.
                    if validate_seen.0 != check_seen.0
                        || validate_seen.1 != check_seen.1
                        || validate_seen.2.is_some() != check_seen.2.is_some()
                    {
                        return Err(format!(
                            "{tag} validate: {validate_seen:?} vs check {check_seen:?}"
                        ));
                    }
                    if let (Ok(cfg), Ok(checked)) = (&full.result, check.result) {
                        let defaulted =
                            schema::config_schema(brand, ext).apply_defaults_owned(checked);
                        if defaulted != cfg.raw {
                            return Err(format!(
                                "{tag} defaults: {defaulted:?} vs raw {:?}",
                                cfg.raw
                            ));
                        }
                    }
                } else {
                    let display = OsStr::new(file);
                    let full = with_opts(brand, ext, |o| {
                        snp_config::load_manifest(&path, display, o).map(|_| ())
                    });
                    let check = with_opts(brand, ext, |o| {
                        snp_config::check_manifest(&path, display, o).map(|_| ())
                    });
                    if seen(&full) != seen(&check) {
                        return Err(format!(
                            "{tag} manifest: load {:?} vs check {:?}",
                            seen(&full),
                            seen(&check)
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn input(c: &Json) -> Option<Vec<u8>> {
        if c.get("missing").is_some() {
            return None;
        }
        if let Some(s) = c["input"].as_str() {
            return Some(s.as_bytes().to_vec());
        }
        let hex = c["input_hex"].as_str().map_or_default(str::as_bytes);
        let digit = |b: u8| char::from(b).to_digit(16).unwrap_or(0);
        Some(
            hex.as_chunks::<2>()
                .0
                .iter()
                .map(|[hi, lo]| u8::try_from(digit(*hi) * 16 + digit(*lo)).unwrap_or(0))
                .collect(),
        )
    }

    /// Every config and manifest case of the pre-commit oracle corpus (`tests/vectors/load.json`).
    #[test]
    fn oracle_corpus_check_equals_load() {
        let cases = vectors("load.json");
        assert!(cases.len() >= 200, "{} cases", cases.len());
        let mut failures = Vec::new();
        for c in &cases {
            let name = c["name"].as_str().expect("name");
            let kind = c["kind"].as_str().expect("kind");
            let file = c["file"].as_str().expect("file");
            if let Err(e) = compare(kind, file, input(c).as_deref()) {
                failures.push(format!("{name}: {e}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// A directory, an over-limit file and an empty file take the same paths in both loads.
    #[test]
    fn read_errors_check_equals_load() {
        for kind in ["config", "manifest"] {
            compare(kind, "empty.yaml", Some(b"")).expect("empty");
            compare(kind, "missing.yaml", None).expect("missing");
            let dir = tempfile::tempdir().expect("tempdir");
            let sub = dir.path().join("sub.yaml");
            std::fs::create_dir_all(&sub).expect("mkdir");
            let full = with_opts(Brand::Snp, ExtMode::Enabled, |o| {
                snp_config::load_manifest(&sub, OsStr::new("sub.yaml"), o).map(|_| ())
            });
            let check = with_opts(Brand::Snp, ExtMode::Enabled, |o| {
                snp_config::check_manifest(&sub, OsStr::new("sub.yaml"), o).map(|_| ())
            });
            assert_eq!(seen(&full), seen(&check));
            assert!(full.result.is_err());
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let big = dir.path().join("big.yaml");
        let f = std::fs::File::create(&big).expect("create");
        f.set_len((16 << 20) + 1).expect("set_len");
        drop(f);
        let check = with_opts(Brand::PreCommit, ExtMode::Enabled, |o| {
            snp_config::check_manifest(&big, OsStr::new("big.yaml"), o).map(|_| ())
        });
        let full = with_opts(Brand::PreCommit, ExtMode::Enabled, |o| {
            snp_config::load_manifest(&big, OsStr::new("big.yaml"), o).map(|_| ())
        });
        assert_eq!(seen(&full), seen(&check));
        assert_eq!(
            check.result.err().map(|e| e.rendered),
            Some(
                "\n==> File big.yaml\n=====> the input is 16777217 bytes long, more than the limit of 16777216 bytes"
                    .to_owned()
            )
        );
    }

    /// Top-level lines of a generated config: valid, warning, failing, and `FutureWarning` regexes.
    const TOP: &[&str] = &[
        "files: '\\.py$'",
        "exclude: '[[a]'",
        "exclude: '('",
        "fail_fast: true",
        "fail_fast: maybe",
        "default_stages: [commit, push]",
        "default_stages: [nope]",
        "minimum_pre_commit_version: '99'",
        "minimum_pre_commit_version: '1.0'",
        "unknown_top: 1",
        "ci: {skip: []}",
        "default_language_version: {python: python3}",
        "default_language_version: [x]",
        "default_install_hook_types: [pre-push]",
        "priority: 3",
    ];

    /// Hook lines of a generated config hook or manifest hook.
    const HOOK: &[&str] = &[
        "name: n",
        "entry: e",
        "language: system",
        "language: python",
        "language: nolang",
        "files: '[[x]'",
        "files: '('",
        "exclude: '(?i)x'",
        "stages: [commit, push]",
        "stages: [bogus]",
        "args: [a, '1']",
        "args: 1",
        "types: [python]",
        "types: [nottag]",
        "types_or: [rust, python]",
        "bogus_key: 1",
        "priority: 3",
        "additional_dependencies: [x==1]",
        "always_run: yes",
        "pass_filenames: false",
        "language_version: default",
        "fail_fast: 1",
    ];

    /// The ids hooks draw from (meta ids included: meta repos validate them).
    const IDS: &[&str] = &[
        "h",
        "check-hooks-apply",
        "identity",
        "check-useless-excludes",
        "bogus-meta",
        "trailing-whitespace",
    ];

    #[derive(Debug, Clone)]
    struct Hook {
        id: Option<usize>,
        lines: Vec<usize>,
    }

    #[derive(Debug, Clone)]
    struct Repo {
        /// 0 remote, 1 remote without rev, 2 local, 3 meta, 4 builtin, 5 not a mapping.
        kind: u8,
        extra: bool,
        hooks: Vec<Hook>,
    }

    fn hook() -> impl Strategy<Value = Hook> {
        (
            prop::option::weighted(0.9, 0..IDS.len()),
            prop::collection::vec(0..HOOK.len(), 0..5),
        )
            .prop_map(|(id, lines)| Hook { id, lines })
    }

    fn repo() -> impl Strategy<Value = Repo> {
        (
            0u8..6,
            prop::bool::weighted(0.15),
            prop::collection::vec(hook(), 0..4),
        )
            .prop_map(|(kind, extra, hooks)| Repo { kind, extra, hooks })
    }

    /// A one-byte corruption: (position permille, replacement byte).
    fn corruption() -> impl Strategy<Value = Option<(usize, u8)>> {
        prop::option::weighted(
            0.2,
            (
                0usize..1000,
                prop::sample::select(vec![b'\xff', b':', b'[', b'\t', b'{', b'&', b'*', b'\'']),
            ),
        )
    }

    fn corrupt(text: &str, c: Option<(usize, u8)>) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        if let Some((permille, b)) = c {
            let i = bytes.len() * permille / 1000;
            if let Some(slot) = bytes.get_mut(i) {
                *slot = b;
            }
        }
        bytes
    }

    fn render_hook(out: &mut String, h: &Hook, indent: &str) {
        let mut first = true;
        let mut line = |out: &mut String, l: &str| {
            let dash = if first { "-   " } else { "    " };
            first = false;
            writeln!(out, "{indent}{dash}{l}").expect("w");
        };
        if let Some(id) = h.id.and_then(|i| IDS.get(i)) {
            line(out, &format!("id: {id}"));
        }
        for l in h.lines.iter().filter_map(|&i| HOOK.get(i)) {
            line(out, l);
        }
        if first {
            writeln!(out, "{indent}-   {{}}").expect("w");
        }
    }

    fn render_config(top: &[usize], repos: &[Repo]) -> String {
        let mut s = String::new();
        for l in top.iter().filter_map(|&i| TOP.get(i)) {
            writeln!(s, "{l}").expect("w");
        }
        s.push_str(if repos.is_empty() {
            "repos: []\n"
        } else {
            "repos:\n"
        });
        for (i, r) in repos.iter().enumerate() {
            match r.kind {
                0 => writeln!(s, "-   repo: https://example.com/r{i}\n    rev: v1.{i}"),
                1 => writeln!(s, "-   repo: https://example.com/r{i}"),
                2 => writeln!(s, "-   repo: local"),
                3 => writeln!(s, "-   repo: meta"),
                4 => writeln!(s, "-   repo: builtin"),
                _ => writeln!(s, "-   just-a-string-{i}"),
            }
            .expect("w");
            if r.kind == 5 {
                continue;
            }
            if r.extra {
                s.push_str("    unexpected_repo_key: x\n");
            }
            s.push_str(if r.hooks.is_empty() {
                "    hooks: []\n"
            } else {
                "    hooks:\n"
            });
            for h in &r.hooks {
                render_hook(&mut s, h, "    ");
            }
        }
        s
    }

    fn render_manifest(hooks: &[Hook], as_map: bool) -> String {
        if as_map {
            return "hooks:\n-   id: h\n".to_owned();
        }
        let mut s = String::new();
        if hooks.is_empty() {
            s.push_str("[]\n");
        }
        for h in hooks {
            render_hook(&mut s, h, "");
        }
        s
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(192))]

        #[test]
        fn generated_configs_check_equals_load(
            top in prop::collection::vec(0..TOP.len(), 0..4),
            repos in prop::collection::vec(repo(), 0..4),
            c in corruption(),
        ) {
            let bytes = corrupt(&render_config(&top, &repos), c);
            prop_assert_eq!(compare("config", "cfg.yaml", Some(&bytes)), Ok(()));
        }

        #[test]
        fn generated_manifests_check_equals_load(
            hooks in prop::collection::vec(hook(), 0..4),
            as_map in prop::bool::weighted(0.05),
            c in corruption(),
        ) {
            let bytes = corrupt(&render_manifest(&hooks, as_map), c);
            prop_assert_eq!(compare("manifest", "m.yaml", Some(&bytes)), Ok(()));
        }
    }

    /// The generators reach every outcome class (so the properties above are not vacuous): a valid config, a
    /// warning, a failing check, and a `FutureWarning`.
    #[test]
    fn generator_reaches_every_outcome() {
        let ok = render_config(
            &[0],
            &[Repo {
                kind: 2,
                extra: false,
                hooks: vec![Hook {
                    id: Some(0),
                    lines: vec![0, 1, 2],
                }],
            }],
        );
        let warns = render_config(&[9], &[]);
        let future = render_config(&[1], &[]);
        let fails = render_config(&[4], &[]);
        let run = |t: &str| {
            let (dir, path) = file_in_tempdir("cfg.yaml", Some(t.as_bytes()));
            let src = ConfigSource {
                path,
                display: "cfg.yaml".into(),
                project_root: dir.path().to_path_buf(),
            };
            seen(&with_opts(Brand::PreCommit, ExtMode::Enabled, |o| {
                snp_config::check_config(&src, o)
            }))
        };
        assert_eq!(run(&ok), (vec![], vec![], None));
        assert_eq!(run(&warns).0.len(), 1);
        assert_eq!(run(&future).1.len(), 1);
        assert!(run(&fails).2.is_some());
        for t in [ok, warns, future, fails] {
            compare("config", "cfg.yaml", Some(t.as_bytes())).expect("equal");
        }
        // The random configs themselves cover each class often enough.
        let mut runner = proptest::test_runner::TestRunner::deterministic();
        let strategy = (
            prop::collection::vec(0..TOP.len(), 0..4),
            prop::collection::vec(repo(), 0..4),
            corruption(),
        );
        let (mut oks, mut errs, mut warned, mut futures) = (0, 0, 0, 0);
        for _ in 0..300 {
            let (top, repos, c) = strategy.new_tree(&mut runner).expect("tree").current();
            let bytes = corrupt(&render_config(&top, &repos), c);
            let s = run(&String::from_utf8_lossy(&bytes));
            if s.2.is_some() {
                errs += 1;
            } else {
                oks += 1;
            }
            warned += usize::from(!s.0.is_empty());
            futures += usize::from(!s.1.is_empty());
        }
        assert!(
            oks >= 15 && errs >= 15 && warned >= 15 && futures >= 15,
            "ok {oks}, err {errs}, warned {warned}, future {futures}"
        );
    }
}
