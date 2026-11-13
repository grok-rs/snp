//! Property tests of the YAML rewrites (research 10 §7.5.5): configs rendered in random styles (indentation, `-   `
//! vs `- `, quotes, comments, blank lines, LF/CRLF/mixed endings, a missing final newline) keep every byte outside
//! the rewritten scalars, are idempotent, and parse to the expected values.
//!
//! Owner: WP-21.

#[cfg(test)]
mod properties {
    use std::ffi::OsStr;
    use std::sync::Arc;

    use proptest::prelude::*;
    use snp_config::migrate::{Outcome, migrate_config};
    use snp_config::revline::{RevUpdate, Rewrite, rewrite};

    /// One repo of a generated config.
    #[derive(Debug, Clone)]
    struct Repo {
        local: bool,
        quote: &'static str,
        comment: Option<&'static str>,
        stages: Vec<&'static str>,
    }

    fn repo() -> impl Strategy<Value = Repo> {
        (
            any::<bool>(),
            prop::sample::select(vec!["", "'", "\""]),
            prop::option::of(prop::sample::select(vec![
                "# pinned",
                "# frozen: v0.9",
                "#x",
            ])),
            prop::collection::vec(
                prop::sample::select(vec!["commit", "push", "pre-commit", "manual"]),
                0..3,
            ),
        )
            .prop_map(|(local, quote, comment, stages)| Repo {
                local,
                quote,
                comment,
                stages,
            })
    }

    /// Renders the config with `indent` spaces, `dash` (`-   ` or `- `), and `eol` per line (index → ending).
    fn render(
        repos: &[Repo],
        indent: usize,
        wide_dash: bool,
        eols: &[bool],
        final_newline: bool,
    ) -> String {
        let pad: String = std::iter::repeat_n(' ', indent).collect();
        let dash = if wide_dash {
            "-   ".to_owned()
        } else {
            "- ".to_owned()
        };
        let inner: String = std::iter::repeat_n(' ', dash.len()).collect();
        let mut lines: Vec<String> = vec!["# header".to_owned(), "repos:".to_owned()];
        for (i, r) in repos.iter().enumerate() {
            if r.local {
                lines.push(format!("{pad}{dash}repo: local"));
            } else {
                lines.push(format!("{pad}{dash}repo: https://example.com/r{i}"));
                let comment = r.comment.map_or_default(|c| format!("  {c}"));
                lines.push(format!(
                    "{pad}{inner}rev: {q}v1.{i}{q}{comment}",
                    q = r.quote
                ));
            }
            lines.push(format!("{pad}{inner}hooks:"));
            lines.push(format!("{pad}{inner}{dash}id: h{i}"));
            if r.local {
                lines.push(format!("{pad}{inner}{inner}name: h"));
                lines.push(format!("{pad}{inner}{inner}entry: h"));
                lines.push(format!("{pad}{inner}{inner}language: system"));
            }
            if !r.stages.is_empty() {
                lines.push(format!(
                    "{pad}{inner}{inner}stages: [{}]",
                    r.stages.join(", ")
                ));
            }
            lines.push(String::new());
        }
        let mut out = String::new();
        let n = lines.len();
        for (i, l) in lines.into_iter().enumerate() {
            out.push_str(&l);
            if i + 1 < n || final_newline {
                out.push_str(if eols.get(i).copied().unwrap_or(false) {
                    "\r\n"
                } else {
                    "\n"
                });
            }
        }
        out
    }

    fn updates(repos: &[Repo], freeze: bool) -> Vec<RevUpdate> {
        repos
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.local)
            .map(|(i, _)| RevUpdate {
                repo_index: i,
                new_rev: Arc::from(format!("v2.{i}")),
                frozen_comment: freeze.then(|| Arc::from("v2.0")),
            })
            .collect()
    }

    fn lines_of(r: Rewrite) -> String {
        match r {
            Rewrite::Lines(b) => String::from_utf8(b).expect("utf8"),
            other => panic!("unexpected reformat: {other:?}"),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn rev_rewrite_touches_only_rev_lines(
            repos in prop::collection::vec(repo(), 1..5),
            indent in 0_usize..3,
            wide in any::<bool>(),
            eols in prop::collection::vec(any::<bool>(), 64),
            final_newline in any::<bool>(),
            freeze in any::<bool>(),
        ) {
            let src = render(&repos, indent + 1, wide, &eols, final_newline);
            let ups = updates(&repos, freeze);
            let out = lines_of(rewrite(src.as_bytes(), &ups).expect("rewrite"));
            // Line structure (and every line ending) is unchanged; only rev lines differ.
            let a: Vec<&str> = src.split_inclusive('\n').collect();
            let b: Vec<&str> = out.split_inclusive('\n').collect();
            prop_assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(&b) {
                if x != y {
                    prop_assert!(x.trim_start().starts_with("rev:"), "{x:?} -> {y:?}");
                    prop_assert_eq!(x.ends_with("\r\n"), y.ends_with("\r\n"));
                }
            }
            prop_assert_eq!(src.ends_with('\n'), out.ends_with('\n'));
            // Idempotent.
            let again = lines_of(rewrite(out.as_bytes(), &ups).expect("rewrite"));
            prop_assert_eq!(&again, &out);
            // The new revs parse back.
            let v = snp_yaml::load(&snp_pycompat::textio::universal_newlines(&out), &snp_yaml::Limits::default()).expect("yaml");
            let parsed = v.as_map().and_then(|m| m.get(&cfgv::Value::from("repos"))).and_then(cfgv::Value::as_array).expect("repos");
            for u in &ups {
                let rev = parsed[u.repo_index].as_map().and_then(|m| m.get(&cfgv::Value::from("rev"))).and_then(cfgv::Value::as_str);
                prop_assert_eq!(rev, Some(&*u.new_rev));
            }
        }

        #[test]
        fn migrate_is_idempotent_and_keeps_line_endings(
            repos in prop::collection::vec(repo(), 1..5),
            wide in any::<bool>(),
            eols in prop::collection::vec(any::<bool>(), 64),
            list_root in any::<bool>(),
        ) {
            let mut src = render(&repos, 1, wide, &eols, true).replace("rev:", "sha:");
            if list_root {
                // The pre-1.0 form: the repos list at the root.
                src = src.replacen("repos:\n", "", 1).replacen("repos:\r\n", "", 1);
                let dedented: Vec<String> = src.split_inclusive('\n').map(|l| l.trim_prefix(' ').to_owned()).collect();
                src = dedented.concat();
            }
            let display = OsStr::new(".pre-commit-config.yaml");
            let out = match migrate_config(src.as_bytes(), display).expect("migrate") {
                Outcome::Migrated(b) => String::from_utf8(b).expect("utf8"),
                Outcome::AlreadyMigrated => src.clone(),
                other => panic!("{other:?}"),
            };
            prop_assert!(!out.contains("sha:"));
            prop_assert!(!out.contains("[commit") && !out.contains(" commit]") && !out.contains(", commit,"));
            prop_assert!(matches!(migrate_config(out.as_bytes(), display).expect("again"), Outcome::AlreadyMigrated));
            prop_assert!(out.matches("\r\n").count() >= src.matches("\r\n").count());
        }
    }
}
