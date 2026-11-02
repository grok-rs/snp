//! Integration tests: the config file name (ADR-087, D-EXT-09, D-DIAG-06): `.snp.yaml` / `.snp.yml` /
//! `.pre-commit-config.yaml` discovery without `-c`, the brand rules, the ignored-file note, the two-snp-names error,
//! the hook scripts' embedded `--config`, the unstaged-config check, the overlay names and config tracking.
//!
//! Owner: WP-33.

use std::path::{Path, PathBuf};

use snp_core::output::CollectWarnings;
use snp_store::{Store, StoreLocation, StoreSource};
use snp_testkit::TestEnv;

use crate::cli::{assert_code, compat, git, snp, store, tenv, text};

/// A local config with one `system` hook named `name`.
fn config(name: &str) -> String {
    format!(
        "repos:\n-   repo: local\n    hooks:\n    -   id: h\n        name: {name}\n        entry: echo\n        language: system\n"
    )
}

/// A committed repository holding `files` (name, contents).
fn repo(env: &TestEnv, name: &str, files: &[(&str, String)]) -> PathBuf {
    let mut b = env.git_repo(name).file("a.txt", b"a\n");
    for (f, c) in files {
        b = b.file(f, c.as_bytes());
    }
    b.stage_all().commit("init").build()
}

const IGNORED_NOTE: &str =
    "snp: using .snp.yaml; ignoring .pre-commit-config.yaml (pass -c/--config to choose)\n";
const MISSING_NOTE: &str =
    "snp: no .snp.yaml or .pre-commit-config.yaml file was found in the repository root\n";

/// The number of times `needle` occurs in `hay`.
fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

#[test]
fn snp_yaml_then_snp_yml_without_c() {
    let env = tenv();
    let r = repo(&env, "yaml", &[(".snp.yaml", config("from-snp-yaml"))]);
    let out = snp(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).starts_with("from-snp-yaml...."),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(text(&out.stderr), "");
    let r = repo(&env, "yml", &[(".snp.yml", config("from-snp-yml"))]);
    let out = snp(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).starts_with("from-snp-yml...."),
        "{}",
        text(&out.stdout)
    );
    // from a subdirectory: the root's file
    std::fs::create_dir_all(r.join("sub")).unwrap();
    let out = snp(&env, &r.join("sub"), &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).starts_with("from-snp-yml...."),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn both_snp_names_is_an_error() {
    let env = tenv();
    let r = repo(
        &env,
        "two",
        &[(".snp.yaml", config("a")), (".snp.yml", config("b"))],
    );
    for out in [
        snp(&env, &r, &["run", "--all-files"]),
        compat(&env, &r, &["run", "--all-files"]),
        snp(&env, &r, &["install"]),
    ] {
        assert_code(&out, 1);
        assert!(
            text(&out.stdout).starts_with(
                "An error has occurred: FatalError: both .snp.yaml and .snp.yml exist; keep one, or \
                 choose with -c/--config\n"
            ),
            "{}",
            text(&out.stdout)
        );
    }
    // `-c` decides
    let out = snp(&env, &r, &["run", "--all-files", "-c", ".snp.yml"]);
    assert_code(&out, 0);
    assert!(text(&out.stdout).starts_with("b...."));
}

#[test]
fn snp_yaml_wins_over_pre_commit_config_with_one_note() {
    let env = tenv();
    let r = repo(
        &env,
        "both",
        &[
            (".snp.yaml", config("from-snp")),
            (".pre-commit-config.yaml", config("from-pcc")),
        ],
    );
    let out = snp(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).starts_with("from-snp...."),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(text(&out.stderr), IGNORED_NOTE);
    // `install --install-hooks` discovers twice and notes once
    let out = snp(&env, &r, &["install", "--install-hooks"]);
    assert_code(&out, 0);
    assert_eq!(
        count(&text(&out.stderr), IGNORED_NOTE),
        1,
        "{}",
        text(&out.stderr)
    );
    // an explicit `-c` wins, without the note
    let out = snp(
        &env,
        &r,
        &["run", "--all-files", "-c", ".pre-commit-config.yaml"],
    );
    assert_code(&out, 0);
    assert!(text(&out.stdout).starts_with("from-pcc...."));
    assert_eq!(text(&out.stderr), "");
    // `SNP_COMPAT_STRICT=1` turns discovery off
    let out = env
        .snp(&r)
        .args(["run", "--all-files"])
        .env("SNP_COMPAT_STRICT", "1")
        .output();
    assert_code(&out, 0);
    assert!(text(&out.stdout).starts_with("from-pcc...."));
}

#[test]
fn compat_brand_prefers_pre_commit_config_silently() {
    let env = tenv();
    let r = repo(
        &env,
        "both",
        &[
            (".snp.yaml", config("from-snp")),
            (".pre-commit-config.yaml", config("from-pcc")),
        ],
    );
    let out = compat(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(text(&out.stdout).starts_with("from-pcc...."));
    assert_eq!(text(&out.stderr), "");
    // without `.pre-commit-config.yaml` the snp file is read
    let r = repo(&env, "snp-only", &[(".snp.yaml", config("from-snp"))]);
    let out = compat(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(text(&out.stdout).starts_with("from-snp...."));
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn no_config_notes_both_names_in_the_snp_brand_only() {
    let env = tenv();
    let r = repo(&env, "none", &[]);
    let missing = "An error has occurred: InvalidConfigError: \n=====> .pre-commit-config.yaml is not a file\n";
    let out = snp(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 1);
    assert!(
        text(&out.stdout).starts_with(missing),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(text(&out.stderr), MISSING_NOTE);
    // pre-commit's bytes in the compat brand
    let out = compat(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 1);
    assert!(
        text(&out.stdout).starts_with(missing),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(text(&out.stderr), "");
    // `install` works without a config: no note
    let out = snp(&env, &r, &["install"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stderr), "");
    // an explicit `-c` names its own file only
    let out = snp(
        &env,
        &r,
        &["run", "--all-files", "-c", ".pre-commit-config.yaml"],
    );
    assert_code(&out, 1);
    assert_eq!(text(&out.stderr), "");
}

/// The `--config=…` of the installed pre-commit script.
fn embedded_config(r: &Path) -> String {
    let script = std::fs::read_to_string(r.join(".git/hooks/pre-commit")).unwrap();
    let start = script.find("--config=").expect("--config in the script");
    let rest = script.get(start..).unwrap_or_default();
    rest.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[cfg(unix)]
#[test]
fn install_embeds_the_discovered_name_and_follows_a_rename() {
    let env = tenv();
    let r = repo(&env, "inst", &[(".snp.yaml", config("from-snp"))]);
    assert_code(&snp(&env, &r, &["install"]), 0);
    assert_eq!(embedded_config(&r), "--config=.snp.yaml");
    std::fs::write(r.join("a.txt"), b"b\n").unwrap();
    assert_eq!(git(&env, &r, &["add", "a.txt"]).0, 0);
    let (code, out, err) = git(&env, &r, &["commit", "-m", "x"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(format!("{out}{err}").contains("from-snp...."), "{out}{err}");
    // renamed: re-installing rewrites the script
    assert_eq!(git(&env, &r, &["mv", ".snp.yaml", ".snp.yml"]).0, 0);
    assert_eq!(
        git(&env, &r, &["commit", "-qm", "rename", "--no-verify"]).0,
        0
    );
    assert_code(&snp(&env, &r, &["install"]), 0);
    assert_eq!(embedded_config(&r), "--config=.snp.yml");
    // the compat brand embeds `.pre-commit-config.yaml` when it exists
    std::fs::write(r.join(".pre-commit-config.yaml"), config("from-pcc")).unwrap();
    assert_code(&compat(&env, &r, &["install"]), 0);
    assert_eq!(embedded_config(&r), "--config=.pre-commit-config.yaml");
}

#[test]
fn unstaged_check_names_the_snp_config() {
    let env = tenv();
    let r = repo(&env, "unstaged", &[(".snp.yaml", config("from-snp"))]);
    std::fs::write(r.join(".snp.yaml"), config("changed")).unwrap();
    let out = snp(&env, &r, &["run"]);
    assert_code(&out, 1);
    assert_eq!(
        text(&out.stdout),
        "[ERROR] Your pre-commit configuration is unstaged.\n`git add .snp.yaml` to fix this.\n"
    );
}

#[test]
fn snp_config_pairs_with_snp_local_overlay() {
    let env = tenv();
    let r = repo(&env, "overlay", &[(".snp.yaml", config("from-snp"))]);
    std::fs::write(
        r.join(".snp.local.yaml"),
        "hooks:\n  h:\n    name: overlaid\n",
    )
    .unwrap();
    std::fs::write(
        r.join(".pre-commit-config.local.yaml"),
        "hooks:\n  h:\n    name: wrong-overlay\n",
    )
    .unwrap();
    let out = snp(&env, &r, &["run", "--all-files"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).starts_with("overlaid...."),
        "{}",
        text(&out.stdout)
    );
    assert!(
        text(&out.stderr).contains(".snp.local.yaml"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn tracking_records_the_snp_config() {
    let env = tenv();
    let r = repo(&env, "tracked", &[(".snp.yaml", config("from-snp"))]);
    let st = Store::new(StoreLocation {
        path: store(&env),
        source: StoreSource::SnpHome,
    });
    st.ensure_created(snp_core::brand::Brand::Snp, &CollectWarnings::new())
        .unwrap();
    assert_code(&snp(&env, &r, &["run", "--all-files"]), 0);
    let tracked = st.tracked_configs();
    assert_eq!(tracked.len(), 1, "{tracked:?}");
    let path = &tracked.first().unwrap().path;
    assert_eq!(path.file_name().unwrap(), ".snp.yaml", "{tracked:?}");
}

#[test]
fn config_commands_use_the_discovered_file() {
    let env = tenv();
    let r = repo(&env, "cmds", &[(".snp.yaml", config("from-snp"))]);
    let out = snp(&env, &r, &["config", "dump"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).contains("name: from-snp"),
        "{}",
        text(&out.stdout)
    );
    let out = snp(&env, &r, &["list"]);
    assert_code(&out, 0);
    assert!(
        text(&out.stdout).contains("from-snp"),
        "{}",
        text(&out.stdout)
    );
    // migrate-config rewrites the discovered file
    std::fs::write(r.join(".snp.yaml"), "-   repo: local\n    hooks: []\n").unwrap();
    let out = snp(&env, &r, &["migrate-config"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "Configuration has been migrated.\n");
    assert_eq!(
        std::fs::read_to_string(r.join(".snp.yaml")).unwrap(),
        "repos:\n-   repo: local\n    hooks: []\n"
    );
    assert!(!r.join(".pre-commit-config.yaml").exists());
}
