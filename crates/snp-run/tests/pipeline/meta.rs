//! Meta hooks (T10-MET, X13-SM-04): in process, with pre-commit's bare warnings and quirks (01 §9).

use crate::harness::{H, all, args, line, local, sys};

fn meta(hooks: &str) -> String {
    format!("-   repo: meta\n    hooks:\n{hooks}")
}

/// The fixture of the pc462-py312 transcript below (bare warnings before the hook's own lines).
#[test]
fn x13_sm_04_bare_warnings_inside_meta_output() {
    let h = H::new();
    h.config_staged(&format!(
        "bogus_root: 1\nrepos:\n{}{}",
        local(&format!(
            "{}{}",
            sys("nofiles", "nofiles", "'true'", "files: ^nothing$\nbogus_key: 1"),
            sys("ex", "ex", "'true'", "exclude: ^zzz"),
        ))
        .trim_start_matches("repos:\n"),
        meta("    -   id: check-hooks-apply\n        verbose: true\n    -   id: check-useless-excludes\n        verbose: true\n"),
    ));
    h.write("a.txt", b"a\n");
    h.stage("a.txt");
    let o = h.run(all());
    assert_eq!(o.code, 1, "{o:?}");
    let got: String = o
        .stdout
        .lines()
        .map(|l| {
            if l.starts_with("- duration: ") {
                "- duration: Xs"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let want = "[WARNING] Unexpected key(s) present at root: bogus_root
[WARNING] Unexpected key(s) present on local => nofiles: bogus_key
nofiles..............................................(no files to check)Skipped
ex.......................................................................Passed
Check hooks apply to the repository......................................Failed
- hook id: check-hooks-apply
- duration: Xs
- exit code: 1

Unexpected key(s) present at root: bogus_root
Unexpected key(s) present on local => nofiles: bogus_key
nofiles does not apply to this repository

Check for useless excludes...............................................Failed
- hook id: check-useless-excludes
- duration: Xs
- exit code: 1

Unexpected key(s) present at root: bogus_root
The exclude pattern '^zzz' for ex does not match any files
";
    assert_eq!(got + "\n", want.to_owned() + "\n");
}

/// `check-hooks-apply` for one extra local hook.
fn hooks_apply(extra: &str) -> H {
    let h = H::new();
    h.config_staged(&format!(
        "repos:\n{}{}",
        meta("    -   id: check-hooks-apply\n"),
        local(extra).trim_start_matches("repos:\n"),
    ));
    h.write("a.py", b"");
    h.stage("a.py");
    h
}

#[test]
fn t10_met_01_check_hooks_apply_reports_itself() {
    for extra in [
        "exclude: .pre-commit-config.yaml",
        "files: foo",
        "types: [python]",
        "exclude_types: [yaml]",
    ] {
        let h = H::new();
        h.config_staged(&format!(
            "repos:\n{}",
            meta(&format!(
                "    -   id: check-useless-excludes\n        {extra}\n    -   id: check-hooks-apply\n"
            ))
        ));
        let o = h.run(all());
        assert_eq!(o.code, 1, "{extra}: {}", o.stdout);
        assert!(
            o.stdout
                .contains("check-useless-excludes does not apply to this repository"),
            "{extra}: {}",
            o.stdout
        );
    }
}

#[test]
fn t10_met_02_fail_and_always_run_hooks_are_exempt() {
    let h = hooks_apply(&format!(
        "{}{}{}",
        sys("y", "yaml", "'true'", "files: \\.yaml$"),
        "    -   id: changelogs-rst\n        name: changelogs must be rst\n        entry: changelog filenames must end in .rst\n        language: fail\n        files: changelog/.*(?<!\\.rst)$\n",
        sys("a", "always", "'true'", "always_run: true\nfiles: ^$"),
    ));
    let o = h.run(all());
    assert_eq!(o.code, 0, "{}", o.stdout);
    assert!(
        o.stdout
            .contains(&line("Check hooks apply to the repository", "Passed"))
    );
}

#[test]
fn t10_met_03_useless_global_exclude() {
    let h = H::new();
    h.config_staged(&format!(
        "exclude: foo\nrepos:\n{}",
        meta("    -   id: check-useless-excludes\n        verbose: true\n")
    ));
    let o = h.run(all());
    assert_eq!(o.code, 1);
    assert!(
        o.stdout
            .contains("\n\nThe global exclude pattern 'foo' does not match any files\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn t10_met_04_useless_hook_exclude() {
    let h = H::new();
    h.config_staged(&format!(
        "repos:\n{}",
        meta("    -   id: check-useless-excludes\n        exclude: foo\n        verbose: true\n")
    ));
    let o = h.run(all());
    assert_eq!(o.code, 1);
    assert!(
        o.stdout.contains(
            "The exclude pattern 'foo' for check-useless-excludes does not match any files"
        )
    );
}

#[test]
fn t10_met_05_exclude_after_type_filter() {
    // the exclude matches the config file, but the type filter (python) removed it first
    let h = H::new();
    h.config_staged(&format!(
        "repos:\n{}{}",
        meta("    -   id: check-useless-excludes\n        verbose: true\n"),
        local(&sys(
            "l",
            "l",
            "'true'",
            "exclude: .pre-commit-config.yaml\ntypes: [python]"
        ))
        .trim_start_matches("repos:\n"),
    ));
    let o = h.run(all());
    assert_eq!(o.code, 1, "{}", o.stdout);
    assert!(
        o.stdout.contains(
            "The exclude pattern '.pre-commit-config.yaml' for l does not match any files"
        )
    );
}

#[test]
fn t10_met_06_no_or_matching_excludes() {
    let h = H::new();
    h.config_staged(&format!(
        "repos:\n{}",
        meta("    -   id: check-useless-excludes\n        exclude: \\.yaml$\n")
    ));
    let o = h.run(all());
    assert_eq!(o.code, 0, "{}", o.stdout);
    let h = H::new();
    h.config_staged(&format!(
        "repos:\n{}",
        meta("    -   id: check-useless-excludes\n")
    ));
    assert_eq!(h.run(all()).code, 0);
}

#[test]
fn t10_met_07_broken_symlink_excluded() {
    let h = H::new();
    h.config_staged(&format!(
        "repos:\n{}{}",
        meta("    -   id: check-useless-excludes\n"),
        local(&sys(
            "l",
            "l",
            "'true'",
            "exclude: ^broken$\ntypes: [symlink]"
        ))
        .trim_start_matches("repos:\n"),
    ));
    std::os::unix::fs::symlink("nowhere", h.path("broken")).unwrap();
    h.stage("broken");
    let o = h.run(all());
    assert_eq!(o.code, 0, "{}", o.stdout);
}

#[test]
fn t10_met_08_identity_prints_its_argv() {
    let h = H::new();
    h.config(&format!(
        "repos:\n{}",
        meta("    -   id: identity\n        args: [a, b, c]\n        always_run: true\n        pass_filenames: false\n")
    ));
    let o = h.run(args());
    assert_eq!(o.code, 0);
    assert!(o.stdout.ends_with("\n\na\nb\nc\n\n"), "{}", o.stdout);
}

#[test]
fn meta_hooks_read_the_default_config_not_dash_c() {
    let h = H::new();
    // `.pre-commit-config.yaml` has a useless exclude; `-c other.yaml` only runs the meta hook
    h.config_staged(&format!(
        "exclude: nomatch\nrepos:\n{}",
        meta("    -   id: identity\n")
    ));
    h.write(
        "other.yaml",
        format!(
            "repos:\n{}",
            meta("    -   id: check-useless-excludes\n        verbose: true\n")
        )
        .as_bytes(),
    );
    h.stage("other.yaml");
    let mut a = all();
    a.config = "other.yaml".into();
    let o = h.run(a);
    assert_eq!(o.code, 1, "{}", o.stdout);
    assert!(
        o.stdout
            .contains("The global exclude pattern 'nomatch' does not match any files")
    );
}

#[test]
fn meta_hooks_are_reentrant_with_one_permit() {
    let mut h = hooks_apply(&sys("py", "py", "'true'", "files: \\.py$"));
    h.env_set("PRE_COMMIT_NO_CONCURRENCY", "1");
    h.jobs = 1;
    let o = h.run(all());
    assert_eq!(o.code, 0, "{}", o.stdout);
}
