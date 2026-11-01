//! Hostile lefthook configs: no value can inject YAML into the output (comments, header), every output loads with
//! pre-commit's loader (snp-yaml `PyYaml11Safe`, ≡ `yaml.CSafeLoader`) and reads back the hooks and values the
//! conversion meant, and resource bombs (`{cmd}` doubling over extends, `only` cross products, alias
//! amplification) end in exit 1 or a lossy note instead of an allocation failure.
//!
//! Owner: WP-34.

#[cfg(test)]
mod robustness {
    use std::fmt::Write as _;
    use std::path::Path;

    use snp_core::failure::Failure;
    use snp_migrate::{Converted, LefthookOpts};

    fn opts(root: &Path, portable: bool) -> LefthookOpts {
        LefthookOpts {
            root: root.to_path_buf(),
            config: None,
            output: None,
            portable,
            local: false,
            strict: false,
        }
    }

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, text) in files {
            let path = d.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        d
    }

    fn convert(files: &[(&str, &str)], portable: bool) -> Converted {
        let d = tree(files);
        snp_migrate::lefthook(&opts(d.path(), portable)).unwrap()
    }

    fn exit_code(f: &Failure) -> (i32, String) {
        match f {
            Failure::SystemExit { code, stderr } => {
                (*code, String::from_utf8_lossy(stderr).into_owned())
            }
            other => panic!("{other:?}"),
        }
    }

    /// A composed node as JSON (scalars as their string values, the way the loader reads string fields).
    fn to_json(doc: &snp_yaml::Document, id: snp_yaml::NodeId) -> serde_json::Value {
        let node = &doc.nodes[usize::try_from(id.0).unwrap()];
        match &node.kind {
            snp_yaml::NodeKind::Scalar(s) => serde_json::Value::String(s.to_string()),
            snp_yaml::NodeKind::Seq(items) => {
                serde_json::Value::Array(items.iter().map(|i| to_json(doc, *i)).collect())
            }
            snp_yaml::NodeKind::Map(pairs) => serde_json::Value::Object(
                pairs
                    .iter()
                    .map(|(k, v)| {
                        (
                            to_json(doc, *k).as_str().unwrap().to_owned(),
                            to_json(doc, *v),
                        )
                    })
                    .collect(),
            ),
            _ => serde_json::Value::Null,
        }
    }

    /// The whole config as pre-commit's loader reads it (and as YAML 1.2, which snp's extension keys use).
    fn loaded(yaml: &str) -> serde_json::Value {
        snp_yaml::load(yaml, &snp_yaml::Limits::default())
            .unwrap_or_else(|e| panic!("unloadable output ({e}):\n{yaml}"));
        snp_yaml::compose(
            yaml,
            snp_yaml::Schema::Yaml12Core,
            &snp_yaml::Limits::default(),
        )
        .unwrap_or_else(|e| panic!("unloadable as YAML 1.2 ({e}):\n{yaml}"));
        let doc = snp_yaml::compose(
            yaml,
            snp_yaml::Schema::PyYaml11Safe,
            &snp_yaml::Limits::default(),
        )
        .unwrap();
        to_json(&doc, doc.root.unwrap())
    }

    /// The hooks of `repos[0]` as pre-commit loads them.
    fn loaded_hooks(yaml: &str) -> Vec<serde_json::Value> {
        let v = loaded(yaml);
        v["repos"]
            .as_array()
            .unwrap()
            .first()
            .map(|r| r["hooks"].as_array().unwrap().clone())
            .unwrap_or_default()
    }

    fn field<'a>(hook: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        hook.get(key)
    }

    fn ids(yaml: &str) -> Vec<String> {
        loaded_hooks(yaml)
            .iter()
            .map(|h| field(h, "id").unwrap().as_str().unwrap().to_owned())
            .collect()
    }

    /// The number of hooks the summary line reports.
    fn summary_hooks(c: &Converted) -> usize {
        let last = c.report.last().unwrap();
        let after = last.split(" into ").nth(1).unwrap();
        after.split(' ').next().unwrap().parse().unwrap()
    }

    const INJECT: &str = "      - id: evil\n        name: evil\n        language: system\n        entry: touch /tmp/pwned\n        always_run: true\n        pass_filenames: false\n      #";

    /// A value quoted in a `# snp-migrate:` note or in the header can never end the comment: YAML's other line
    /// breaks (U+0085, U+2028, U+2029) become spaces, and non-printable characters are escaped.
    #[test]
    fn notes_never_inject_yaml() {
        let mut cases: Vec<(String, String)> = Vec::new();
        for brk in ["\u{85}", "\u{2028}", "\u{2029}", "\r", "\n"] {
            let payload = INJECT.replace('\n', brk);
            let p = serde_json::to_string(&format!("q{brk}{payload}")).unwrap();
            cases.push((
                "skip condition".to_owned(),
                format!(
                    r#"{{"pre-commit":{{"jobs":[{{"name":"j","run":"echo hi","skip":[{p}]}}]}}}}"#
                ),
            ));
            cases.push((
                "file type".to_owned(),
                format!(r#"{{"pre-commit":{{"jobs":[{{"name":"j","run":"echo hi","file_types":[{p}]}}]}}}}"#),
            ));
            cases.push((
                "tag".to_owned(),
                format!(
                    r#"{{"pre-commit":{{"jobs":[{{"name":"j","run":"echo hi","tags":[{p}]}}]}}}}"#
                ),
            ));
            cases.push((
                "script".to_owned(),
                format!(r#"{{"pre-commit":{{"jobs":[{{"name":"j","script":{p}}}]}}}}"#),
            ));
            cases.push((
                "remote".to_owned(),
                format!(
                    r#"{{"remotes":[{{"git_url":{p}}}],"pre-commit":{{"jobs":[{{"name":"j","run":"echo hi"}}]}}}}"#
                ),
            ));
            cases.push((
                "unknown key".to_owned(),
                format!(r#"{{{p}:1,"pre-commit":{{"jobs":[{{"name":"j","run":"echo hi"}}]}}}}"#),
            ));
            cases.push((
                "exclude under root".to_owned(),
                format!(
                    r#"{{"pre-commit":{{"jobs":[{{"name":"j","run":"echo {{staged_files}}","root":"sub","exclude":[{p}]}}]}}}}"#
                ),
            ));
        }
        for (what, json) in &cases {
            for portable in [false, true] {
                let c = convert(&[("lefthook.json", json)], portable);
                let got = ids(&c.config_yaml);
                assert!(
                    !got.iter().any(|i| i == "evil"),
                    "{what}: {}",
                    c.config_yaml
                );
                assert_eq!(got.len(), summary_hooks(&c), "{what}: {}", c.config_yaml);
                for line in c.config_yaml.lines() {
                    assert!(!line.trim_start().starts_with("- id: evil"), "{what}");
                }
            }
        }
    }

    /// The reviewers' NEL repro: a missing script's name carries a whole hook.
    #[test]
    fn missing_script_name_with_nel() {
        let json = r#"{"pre-commit":{"jobs":[{"name":"a","script":"x\u0085      - id: injected\u0085        name: injected\u0085        language: system\u0085        entry: echo PWNED\u0085        stages: [pre-commit]\u0085      #"}]}}"#;
        for portable in [false, true] {
            let c = convert(&[("lefthook.json", json)], portable);
            assert!(!ids(&c.config_yaml).iter().any(|i| i == "injected"));
        }
    }

    /// An extends file name with a line break goes into the header comment escaped.
    #[cfg(unix)]
    #[test]
    fn header_file_names_stay_comments() {
        let name = "e\nminimum_pre_commit_version: '99'\n#.yml";
        let c = convert(
            &[
                (
                    "lefthook.yml",
                    "extends: ['e*']\npre-commit:\n  jobs: [{run: echo hi}]\n",
                ),
                (name, "pre-push:\n  jobs: [{run: echo ext}]\n"),
            ],
            false,
        );
        let v = loaded(&c.config_yaml);
        assert!(
            v.get("minimum_pre_commit_version").is_none(),
            "{}",
            c.config_yaml
        );
        assert_eq!(ids(&c.config_yaml).len(), 2, "{}", c.config_yaml);
    }

    /// Characters PyYAML refuses to read (U+FFFE, U+FFFF, DEL in a comment, C1 controls) are escaped.
    #[test]
    fn unreadable_characters_are_escaped() {
        for s in [
            "a\u{ffff}b",
            "a\u{fffe}b",
            "a\u{7f}",
            "a\u{9b}b",
            "a\u{1b}[2Jb",
            "a\u{feff}b",
        ] {
            let n = serde_json::to_string(s).unwrap();
            let json = format!(
                r#"{{"pre-commit":{{"jobs":[{{"name":{n},"run":"echo hi","env":{{"K":{n}}},"fail_text":{n},"skip":[{n}]}}]}}}}"#
            );
            for portable in [false, true] {
                let c = convert(&[("lefthook.json", &json)], portable);
                let hooks = loaded_hooks(&c.config_yaml);
                assert_eq!(hooks.len(), 1, "{s:?}");
                assert_eq!(field(&hooks[0], "name").unwrap().as_str(), Some(s), "{s:?}");
                if !portable {
                    let env = field(&hooks[0], "env").unwrap();
                    assert_eq!(env["K"].as_str(), Some(s));
                    assert_eq!(field(&hooks[0], "fail_text").unwrap().as_str(), Some(s));
                }
                // Report lines (printed to a terminal) hold no control characters.
                for line in &c.report {
                    assert!(
                        !line.chars().any(|c| c.is_control() && c != '\t'),
                        "{line:?}"
                    );
                }
            }
        }
        // The fuzz crash input (`\x01` + JSON): portable YAML with a U+FFFF name.
        let c = convert(
            &[(
                "lefthook.json",
                "{\"pre-commit\":{\"jobs\":[{\"name\":\"a\u{ffff}b\",\"run\":\"echo\"}]}}",
            )],
            true,
        );
        assert_eq!(ids(&c.config_yaml).len(), 1);
    }

    /// Load errors quoting a value never print control characters either.
    #[test]
    fn error_messages_escape_controls() {
        let d = tree(&[(
            "lefthook.json",
            "{\"pre-commit\":{\"jobs\":[{\"name\":\"a\",\"run\":\"x\",\"timeout\":{\"\\u001b[2J\":1}}]}}",
        )]);
        if let Err(e) = snp_migrate::lefthook(&opts(d.path(), false)) {
            let (_, text) = exit_code(&e);
            assert!(!text.trim_end().chars().any(char::is_control), "{text:?}");
        }
    }

    fn hostile() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        proptest::collection::vec(
            prop_oneof![
                Just('a'),
                Just(' '),
                Just('\t'),
                Just('\n'),
                Just('\r'),
                Just('\u{85}'),
                Just('\u{2028}'),
                Just('\u{2029}'),
                Just('\u{7f}'),
                Just('\u{1b}'),
                Just('\u{0}'),
                Just('\u{9c}'),
                Just('\u{a0}'),
                Just('\u{fffe}'),
                Just('\u{ffff}'),
                Just('\u{feff}'),
                Just('\u{1f600}'),
                Just('#'),
                Just(':'),
                Just('-'),
                Just('"'),
                Just('\''),
                Just('{'),
                Just('}'),
                Just('['),
                Just(','),
                Just('&'),
                Just('*'),
                Just('!'),
                Just('|'),
                Just('>'),
                Just('%'),
                Just('@'),
                Just('`'),
                Just('é'),
            ],
            0..12,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(300))]
        /// Every output loads with pre-commit's loader; the hook list matches the summary, names and env values read
        /// back unchanged, and no value adds a hook.
        #[test]
        fn outputs_read_back(
            name in hostile(),
            env in hostile(),
            cond in hostile(),
            tag in hostile(),
            ft in hostile(),
            text in hostile(),
            run in hostile(),
            portable in proptest::bool::ANY,
        ) {
            let name = format!("n{name}");
            let env = env.replace('$', "");
            let job = serde_json::json!({
                "name": name,
                "run": format!("echo {run}"),
                "env": {"K": env},
                "skip": [cond],
                "tags": [tag],
                "file_types": [ft],
                "fail_text": text,
                "exclude": [cond],
            });
            let other = serde_json::json!({"name": "other", "run": "true", "script": text});
            let cfg = serde_json::json!({
                "pre-commit": {"jobs": [job, other]},
                "remotes": [{"git_url": cond}],
                text: 1,
            });
            let d = tree(&[("lefthook.json", &cfg.to_string())]);
            let c = match snp_migrate::lefthook(&opts(d.path(), portable)) {
                Ok(c) => c,
                Err(e) => {
                    let (code, _) = exit_code(&e);
                    proptest::prop_assert_eq!(code, 1);
                    return Ok(());
                }
            };
            let hooks = loaded_hooks(&c.config_yaml);
            proptest::prop_assert_eq!(hooks.len(), summary_hooks(&c), "{}", c.config_yaml);
            proptest::prop_assert!(hooks.len() <= 2, "{}", c.config_yaml);
            let first = &hooks[0];
            // A job's own name is kept verbatim.
            proptest::prop_assert_eq!(field(first, "name").and_then(|v| v.as_str()), Some(name.as_str()));
            if !portable && let Some(e) = field(first, "env") {
                proptest::prop_assert_eq!(e["K"].as_str(), Some(env.as_str()));
            }
            for line in &c.report {
                proptest::prop_assert!(!line.chars().any(|c| c.is_control() && c != '\t'), "{:?}", line);
            }
        }
    }

    /// `run: '{cmd}{cmd}'` in 40 extends files doubles the command each time: the load stops with exit 1.
    #[test]
    fn cmd_doubling_over_extends_is_bounded() {
        let mut files: Vec<(String, String)> = vec![(
            "lefthook.yml".to_owned(),
            format!(
                "extends: [{}]\npre-commit:\n  jobs:\n    - {{name: x, run: 'echo 0123456789'}}\n",
                (0..40)
                    .map(|i| format!("e/{i:02}.yml"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )];
        for i in 0..40 {
            files.push((
                format!("e/{i:02}.yml"),
                "pre-commit: {jobs: [{name: x, run: '{cmd}{cmd}'}]}\n".to_owned(),
            ));
        }
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let d = tree(&refs);
        let (code, text) = exit_code(&snp_migrate::lefthook(&opts(d.path(), false)).unwrap_err());
        assert_eq!(code, 1);
        assert!(text.contains("`{cmd}` expansion of job 'x'"), "{text}");
        // A handful of levels still merges.
        let c = convert(
            &[
                (
                    "lefthook.yml",
                    "extends: [a.yml]\npre-commit:\n  jobs: [{name: x, run: lint}]\n",
                ),
                (
                    "a.yml",
                    "pre-commit: {jobs: [{name: x, run: 'nice {cmd}'}]}\n",
                ),
            ],
            false,
        );
        assert!(
            c.config_yaml.contains("entry: nice lint"),
            "{}",
            c.config_yaml
        );
    }

    /// 13 nested `only` levels of 4 items: a lossy note, not 4^13 maps.
    #[test]
    fn nested_only_levels_are_bounded() {
        let mut yaml = String::from(
            "pre-commit:\n  only: [{ref: a0}, {ref: a1}, {ref: a2}, {ref: a3}]\n  jobs:\n",
        );
        let mut indent = String::from("    ");
        for level in 0..12 {
            write!(
                yaml,
                "{indent}- group:\n{indent}    only: [{{ref: b{level}}}, {{ref: c{level}}}, {{ref: d{level}}}, {{ref: e{level}}}]\n{indent}    jobs:\n"
            )
            .unwrap();
            indent.push_str("      ");
        }
        writeln!(
            yaml,
            "{indent}- {{name: leaf, run: echo, only: [{{ref: z}}]}}"
        )
        .unwrap();
        let c = convert(&[("lefthook.yml", &yaml)], false);
        assert!(c.lossy, "{:?}", c.report);
        assert_eq!(ids(&c.config_yaml).len(), 1);
    }

    /// 5 000 aliases of a 3 MiB scalar (a 3 MiB file): exit 1, not 15 GB of copies.
    #[test]
    fn alias_amplification_is_refused() {
        let big: String = std::iter::repeat_n('a', 3 << 20).collect();
        let aliases = vec!["*x"; 5000].join(", ");
        let yaml =
            format!("min_version: &x {big}\nfoo: [{aliases}]\npre-commit:\n  jobs: [{{run: x}}]\n");
        let d = tree(&[("lefthook.yml", &yaml)]);
        let (code, text) = exit_code(&snp_migrate::lefthook(&opts(d.path(), false)).unwrap_err());
        assert_eq!(code, 1);
        assert!(text.contains("expands (through aliases)"), "{text}");
        // Small aliases are fine.
        let c = convert(
            &[(
                "lefthook.yml",
                "x: &r {run: echo hi}\npre-commit:\n  commands:\n    a: *r\n    b: *r\n",
            )],
            false,
        );
        assert_eq!(ids(&c.config_yaml).len(), 2);
    }

    /// Extends paths are globbed relative to the repository: metacharacters in the repository's own path are not
    /// a pattern.
    #[test]
    fn extends_under_a_root_with_metacharacters() {
        let d = tempfile::tempdir().unwrap();
        // `*`, `?` and `\\` cannot appear in a Windows file name; `[` can.
        let dirs: &[&str] = if cfg!(windows) {
            &["m[1]"]
        } else {
            &["m[1]", "m*", "m?x", "m\\y"]
        };
        for dir in dirs {
            let root = d.path().join(dir);
            std::fs::create_dir_all(root.join("e")).unwrap();
            std::fs::write(
                root.join("lefthook.yml"),
                "extends: [e/a.yml, 'e/*.yaml']\npre-commit:\n  jobs: [{run: echo main}]\n",
            )
            .unwrap();
            std::fs::write(
                root.join("e/a.yml"),
                "pre-push:\n  jobs: [{run: echo fromext}]\n",
            )
            .unwrap();
            std::fs::write(
                root.join("e/b.yaml"),
                "post-merge:\n  jobs: [{run: echo globbed}]\n",
            )
            .unwrap();
            let c = snp_migrate::lefthook(&opts(&root, false)).unwrap();
            assert!(
                c.config_yaml.contains("echo fromext"),
                "{dir}: {}",
                c.config_yaml
            );
            assert!(
                c.config_yaml.contains("echo globbed"),
                "{dir}: {}",
                c.config_yaml
            );
            assert!(
                c.config_yaml.starts_with(
                    "# Generated by `snp migrate lefthook` from lefthook.yml, e/a.yml, e/b.yaml\n"
                ),
                "{dir}: {}",
                c.config_yaml
            );
        }
    }

    /// `--strict --portable` exits 2 on the behaviour changes of portable mode.
    #[test]
    fn portable_behaviour_changes_are_lossy() {
        for (what, yaml, msg) in [
            (
                "fail_on_changes",
                "pre-commit:\n  commands:\n    a: {run: x}\n",
                "lossy: lefthook.yml: pre-commit fails hooks that modify files; lefthook did not",
            ),
            (
                "fail_text",
                "pre-commit:\n  fail_on_changes: always\n  commands:\n    a: {run: x, fail_text: hint}\n",
                "lossy: pre-commit/a: fail_text dropped (portable mode)",
            ),
            (
                "tags",
                "pre-commit:\n  fail_on_changes: always\n  commands:\n    a: {run: x, tags: [t]}\n",
                "lossy: pre-commit/a: tags dropped (portable mode)",
            ),
        ] {
            let d = tree(&[("lefthook.yml", yaml)]);
            let mut o = opts(d.path(), true);
            let c = snp_migrate::lefthook(&o).unwrap();
            assert!(
                c.report.iter().any(|l| l.contains(msg)),
                "{what}: {:?}",
                c.report
            );
            assert_eq!(
                c.report.iter().filter(|l| l.contains(": lossy: ")).count(),
                1,
                "{what}: {:?}",
                c.report
            );
            o.strict = true;
            let (code, _) = exit_code(&snp_migrate::lefthook(&o).unwrap_err());
            assert_eq!(code, 2, "{what}");
        }
    }

    /// Two directory symlinks to `.` make `*/*/…/x.yml` match 2^n paths: the glob budget stops the load with
    /// exit 1 within a fraction of a second instead of walking for hours (lefthook has no bound).
    #[cfg(unix)]
    #[test]
    fn symlink_loop_extends_glob_is_bounded() {
        let pattern = format!("{}x.yml", std::iter::repeat_n("*/", 24).collect::<String>());
        let d = tree(&[
            ("lefthook.yml", &format!("extends: ['{pattern}']\n")),
            ("x.yml", "pre-commit: {jobs: [{name: x, run: lint}]}\n"),
        ]);
        std::os::unix::fs::symlink(".", d.path().join("a")).unwrap();
        std::os::unix::fs::symlink(".", d.path().join("b")).unwrap();
        let t = std::time::Instant::now();
        let (code, text) = exit_code(&snp_migrate::lefthook(&opts(d.path(), false)).unwrap_err());
        assert_eq!(code, 1);
        assert!(
            text.contains(&format!(
                "extends pattern '{pattern}' matches too many paths"
            )),
            "{text}"
        );
        assert!(
            t.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            t.elapsed()
        );
        // A shallow pattern through the same links still loads.
        std::fs::write(d.path().join("lefthook.yml"), "extends: ['a/*.yml']\n").unwrap();
        let c = snp_migrate::lefthook(&opts(d.path(), false)).unwrap();
        assert!(c.config_yaml.contains("entry: lint"), "{}", c.config_yaml);
    }

    /// A FIFO reached by `extends` is refused instead of blocking the read forever.
    #[cfg(unix)]
    #[test]
    fn fifo_extends_is_refused() {
        let d = tree(&[(
            "lefthook.yml",
            "extends: [e.yml]\npre-commit: {jobs: [{run: lint}]}\n",
        )]);
        let fifo = d.path().join("e.yml");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            return;
        }
        let o = opts(d.path(), false);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .spawn(move || drop(tx.send(snp_migrate::lefthook(&o).map(|_| ()))))
            .unwrap();
        let r = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the load blocked on the FIFO");
        let (code, text) = exit_code(&r.unwrap_err());
        assert_eq!(code, 1);
        assert!(text.contains("e.yml: not a regular file"), "{text}");
    }

    /// The R5 `{files}` list follows lefthook's line handling: git prints non-ASCII names unquoted
    /// (`core.quotePath=false`) and each line is trimmed (`strings.TrimSpace`) before the existence check.
    #[cfg(unix)]
    #[test]
    fn files_list_keeps_non_ascii_names_and_trims_lines() {
        let d = tree(&[
            (
                "lefthook.yml",
                "pre-commit:\n  jobs:\n    - {name: tracked, files: git ls-files, run: 'printf \"[%s]\" {files}'}\n    - {name: spaced, files: \"printf ' plain.txt \\\\r\\\\n'\", run: 'printf \"[%s]\" {files}'}\n",
            ),
            ("caf\u{e9}.txt", ""),
            (" sp.txt", ""),
            ("plain.txt", ""),
        ]);
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(d.path())
                .env_remove("GIT_CONFIG_COUNT")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "core.quotePath=true",
            "add",
            "caf\u{e9}.txt",
            " sp.txt",
            "plain.txt",
        ]);
        let c = snp_migrate::lefthook(&opts(d.path(), false)).unwrap();
        let hooks = loaded_hooks(&c.config_yaml);
        let mut got = Vec::new();
        for h in &hooks {
            let entry = field(h, "entry").unwrap().as_str().unwrap();
            // pre-commit's `shlex.split(entry)` equals `sh`'s parse of the `sh -c '…' --` wrapper.
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(entry)
                .current_dir(d.path())
                .env_remove("GIT_CONFIG_COUNT")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{entry}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            got.push(String::from_utf8(out.stdout).unwrap());
        }
        // lefthook: café.txt unquoted, ' sp.txt' trimmed to a name that does not exist.
        assert_eq!(
            got,
            ["[caf\u{e9}.txt][plain.txt]", "[plain.txt]"],
            "{}",
            c.config_yaml
        );
    }

    /// A glob with many stars becomes a regex Python's backtracking `re` runs slowly: a warning (not lossy), and a
    /// realistic glob has none.
    #[test]
    fn many_star_globs_warn_about_slow_regexes() {
        let c = convert(
            &[(
                "lefthook.yml",
                "pre-commit:\n  jobs:\n    - {name: slow, glob: '*a*a*a*a*a*a*a*a*a*a*b', run: lint}\n    - {name: ok, glob: '**/*.{js,ts}', run: lint}\n",
            )],
            false,
        );
        let warns: Vec<&String> = c
            .report
            .iter()
            .filter(|l| l.contains("unbounded runs"))
            .collect();
        assert_eq!(warns.len(), 1, "{:?}", c.report);
        assert!(
            warns[0].contains("pre-commit/slow: the glob regex has 11 unbounded runs"),
            "{warns:?}"
        );
    }
}
