//! pre-commit's `testing/resources` repositories and conftest scenarios (10 §3, §7.3; `testing/fixtures.py`,
//! `tests/conftest.py`, `tests/commands/{hook_impl,autoupdate}_test.py`, `tests/staged_files_only_test.py`).
//!
//! The resources are recreated verbatim under `crates/snp-testkit/resources/<name>/` (pre-commit, MIT, see
//! `resources/LICENSE.pre-commit`). YAML is written with the exact bytes pre-commit's `yaml_dump` produces (checked
//! against PyYAML 6.0.3 through the oracle).
//!
//! Owner: WP-18.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::TestEnv;
use crate::harness::{OrFail, copy_tree, fail, git, write};

/// One variant per repository of pre-commit's `testing/resources` (10 §3, recreated verbatim under
/// `crates/snp-testkit/resources/<name>/`), plus the conftest scenario helpers of 10 §7.3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fixture {
    /// e.g. "`script_hooks_repo`", "`python_hooks_repo`" (10 §3 names): `make_repo(<name>)` — the files copied
    /// into a fresh `master` repository, committed as `make_repo`.
    Resource(&'static str),
    /// `in_merge_conflict`: the clone of a consuming repo of `script_hooks_repo` stopped in `git merge foo`.
    MergeConflict,
    /// `in_conflicting_submodule`: the submodule `sub` of a repository, stopped in `git merge foo` (returns the
    /// submodule's path).
    ConflictingSubmodule,
    /// `commit_msg_repo`.
    CommitMsgRepo,
    /// `prepare_commit_msg_repo` (`failing_prepare_commit_msg_repo`).
    PrepareCommitMsgRepo {
        /// The failing variant.
        failing: bool,
    },
    /// `push_example`: a clone one commit ahead of its `origin` (returns the clone).
    PushExample,
    /// `make_consuming_repo(<resource>)`: a repository whose `.pre-commit-config.yaml` uses every hook of the
    /// resource at its `HEAD` through `file://`.
    ConsumingRepo(&'static str),
    /// `out_of_date` (`tagged_repo` variant): `python_hooks_repo` plus one commit (tagged `v1.2.3`).
    OutOfDate {
        /// The upstream has a tag.
        tagged: bool,
    },
    /// `hook_disappearing`: `python_hooks_repo` whose manifest's hook was renamed to `bar`.
    HookDisappearing,
    /// `img_staged`: `img.jpg` (= `img1.jpg`) staged in a fresh repository.
    ImgStaged,
    /// `sub_staged`: a submodule `sub` (at the first of two commits) staged in a repository.
    SubStaged,
}

/// The resources directory of this crate.
pub(crate) fn resources() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources")
}

/// The resource repositories of 10 §3.
#[cfg(test)]
const RESOURCES: [&str; 15] = [
    "arbitrary_bytes_repo",
    "arg_per_line_hooks_repo",
    "exclude_types_repo",
    "failing_hook_repo",
    "logfile_repo",
    "modified_file_returns_zero_repo",
    "not_found_exe",
    "prints_cwd_repo",
    "python_hooks_repo",
    "script_hooks_repo",
    "stdout_stderr_repo",
    "types_or_repo",
    "types_repo",
    // single files of `testing/resources` (used by `img_staged` and the binary stash tests)
    "img1.jpg",
    "img2.jpg",
];

/// Scripts committed as 100755 (10 §3: every `*.sh` and entry script).
fn is_script(rel: &Path) -> bool {
    let name = rel
        .file_name()
        .map_or_else(Default::default, OsStr::to_string_lossy);
    name.ends_with(".sh") || name.ends_with("-entry")
}

struct Ctx<'a> {
    env: &'a TestEnv,
}

impl Ctx<'_> {
    fn git<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> Vec<u8> {
        git(self.env.env(), cwd, args, &self.env.dates)
    }

    fn text<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> String {
        String::from_utf8_lossy(&self.git(cwd, args))
            .trim()
            .to_owned()
    }

    /// `git_dir(tempdir_factory)`: `git -c init.defaultBranch=master init <path>`.
    fn git_dir(&self, base: &str) -> PathBuf {
        let p = self.env.fresh_dir(base);
        self.git(&p, &["-c", "init.defaultBranch=master", "init", "-q", "."]);
        p
    }

    /// `testing.util.git_commit`: `git commit --allow-empty --no-gpg-sign -a -m <msg>`.
    fn commit(&self, repo: &Path, msg: &str) {
        self.git(
            repo,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "--no-gpg-sign",
                "-a",
                "-m",
                msg,
            ],
        );
    }

    fn head(&self, repo: &Path) -> String {
        self.text(repo, &["rev-parse", "HEAD"])
    }

    /// `make_repo(tempdir_factory, repo_source)`.
    fn make_repo(&self, source: &str) -> PathBuf {
        let src = resources().join(source);
        if !src.is_dir() {
            fail(&format!("unknown testing/resources repository {source:?}"));
        }
        let path = self.git_dir(source);
        let files = copy_tree(&src, &path);
        self.git(&path, &["add", "."]);
        for f in files.iter().filter(|f| is_script(f)) {
            // Windows has no exec bit to pick up: the index mode is set explicitly (a no-op elsewhere).
            self.git(
                &path,
                &[
                    OsStr::new("update-index"),
                    OsStr::new("--chmod=+x"),
                    OsStr::new("--"),
                    f.as_os_str(),
                ],
            );
        }
        self.commit(&path, "make_repo");
        path
    }

    /// `make_consuming_repo(tempdir_factory, repo_source)` (+ `make_config_from_repo`, `add_config_to_repo`).
    fn consuming_repo(&self, source: &str) -> PathBuf {
        let hooks = self.make_repo(source);
        let manifest = std::fs::read_to_string(hooks.join(".pre-commit-hooks.yaml"))
            .or_fail("reading the resource manifest");
        let ids: Vec<&str> = manifest
            .lines()
            .filter_map(|l| l.strip_prefix("-   id: "))
            .map(str::trim)
            .collect();
        let rev = self.head(&hooks);
        let url = format!("file://{}", hooks.to_string_lossy().replace('\\', "/"));
        let mut yaml = format!(
            "repos:\n-   repo: {}\n    rev: {rev}\n    hooks:\n",
            yaml_scalar(&url)
        );
        for id in ids {
            yaml.push_str("    -   id: ");
            yaml.push_str(&yaml_scalar(id));
            yaml.push('\n');
        }
        let path = self.git_dir(&format!("{source}-consumer"));
        write(&path.join(".pre-commit-config.yaml"), yaml.as_bytes());
        self.git(&path, &["add", ".pre-commit-config.yaml"]);
        self.commit(&path, "add_config_to_repo");
        path
    }

    /// `_make_conflict` (`tests/conftest.py:53-70`), in `repo`.
    fn make_conflict(&self, repo: &Path) {
        self.git(repo, &["checkout", "-q", "origin/master", "-b", "foo"]);
        write(&repo.join("conflict_file"), b"herp\nderp\n");
        self.git(repo, &["add", "conflict_file"]);
        write(&repo.join("foo_only_file"), b"foo");
        self.git(repo, &["add", "foo_only_file"]);
        self.commit(repo, "_make_conflict");
        self.git(repo, &["checkout", "-q", "origin/master", "-b", "bar"]);
        write(&repo.join("conflict_file"), b"harp\nddrp\n");
        self.git(repo, &["add", "conflict_file"]);
        write(&repo.join("bar_only_file"), b"bar");
        self.git(repo, &["add", "bar_only_file"]);
        self.commit(repo, "_make_conflict");
        let (ok, _) = crate::harness::git_full(
            self.env.env(),
            repo,
            &["merge", "foo"],
            &self.env.dates,
            true,
        );
        if ok {
            fail("`git merge foo` was expected to conflict");
        }
    }

    /// A local-hook repository (`commit_msg_repo` and friends): config committed with `git add .`.
    fn local_config_repo(
        &self,
        base: &str,
        yaml: &str,
        msg: &str,
        extra: Option<(&str, &[u8])>,
    ) -> PathBuf {
        let path = self.git_dir(base);
        write(&path.join(".pre-commit-config.yaml"), yaml.as_bytes());
        if let Some((name, bytes)) = extra {
            let p = path.join(name);
            write(&p, bytes);
            crate::harness::make_executable(&p);
        }
        self.git(&path, &["add", "."]);
        if let Some((name, _)) = extra {
            self.git(&path, &["update-index", "--chmod=+x", "--", name]);
        }
        self.commit(&path, msg);
        path
    }
}

/// PyYAML's plain scalar when it stays plain, else its single-quoted form (`'` doubled). Plain requires: no leading
/// indicator, no `: ` / ` #`, no trailing `:`/space, printable ASCII (what `yaml_dump` emits for these values; the
/// fixtures only ever see ASCII temp paths and hook ids).
fn yaml_scalar(s: &str) -> String {
    let plain_ok = !s.is_empty()
        && !s.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@` ".contains(c))
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.ends_with([':', ' '])
        && s.bytes().all(|b| (0x20..0x7f).contains(&b));
    if plain_ok {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

/// The `commit_msg_repo` config (`yaml_dump`, verified with the oracle).
const COMMIT_MSG_CONFIG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: must-have-signoff\n        name: Must \
have \"Signed off by:\"\n        entry: grep -q \"Signed off by:\"\n        language: system\n        stages:\n        - \
commit-msg\n";
/// The `prepare_commit_msg_repo` config.
const PREPARE_CONFIG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: add-signoff\n        name: Add \"Signed \
off by:\"\n        entry: ./add_sign_off.sh\n        language: script\n        stages:\n        - prepare-commit-msg\n";
/// The `failing_prepare_commit_msg_repo` config.
const FAILING_PREPARE_CONFIG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: add-signoff\n        name: Add \
\"Signed off by:\"\n        entry: bash -c \"exit 1\"\n        language: system\n        stages:\n        - \
prepare-commit-msg\n";
/// `add_sign_off.sh`.
const ADD_SIGN_OFF: &[u8] = b"#!/usr/bin/env bash\nset -eu\necho \"\nSigned off by: \" >> \"$1\"\n";
/// `python_hooks_repo`'s manifest after `manifest[0]['id'] = 'bar'` and `yaml_dump` (`hook_disappearing`).
const HOOK_DISAPPEARING_MANIFEST: &[u8] =
    b"-   id: bar\n    name: Foo\n    entry: foo\n    language: python\n    files: \\.py$\n";

/// Builds `fixture` under `env`'s root and returns the path its test works in.
pub(crate) fn build(env: &TestEnv, fixture: Fixture) -> PathBuf {
    let cx = Ctx { env };
    match fixture {
        Fixture::Resource(name) => cx.make_repo(name),
        Fixture::ConsumingRepo(name) => cx.consuming_repo(name),
        Fixture::MergeConflict => {
            let path = cx.consuming_repo("script_hooks_repo");
            write(&path.join("placeholder"), b"");
            cx.git(&path, &["add", "placeholder"]);
            cx.commit(&path, "in_merge_conflict");
            let conflict = env.fresh_dir("in_merge_conflict");
            cx.git(
                &conflict,
                &[
                    OsStr::new("clone"),
                    OsStr::new("-q"),
                    path.as_os_str(),
                    OsStr::new("."),
                ],
            );
            cx.make_conflict(&conflict);
            conflict
        }
        Fixture::ConflictingSubmodule => {
            let one = cx.git_dir("in_conflicting_submodule");
            let two = cx.git_dir("in_conflicting_submodule");
            cx.commit(&two, "in_conflicting_submodule");
            cx.git(
                &one,
                &[
                    OsStr::new("submodule"),
                    OsStr::new("add"),
                    OsStr::new("-q"),
                    two.as_os_str(),
                    OsStr::new("sub"),
                ],
            );
            let sub = one.join("sub");
            cx.make_conflict(&sub);
            sub
        }
        Fixture::CommitMsgRepo => cx.local_config_repo(
            "commit_msg_repo",
            COMMIT_MSG_CONFIG,
            "commit_msg_repo",
            None,
        ),
        Fixture::PrepareCommitMsgRepo { failing: false } => cx.local_config_repo(
            "prepare_commit_msg_repo",
            PREPARE_CONFIG,
            "prepare_commit_msg_repo",
            Some(("add_sign_off.sh", ADD_SIGN_OFF)),
        ),
        Fixture::PrepareCommitMsgRepo { failing: true } => cx.local_config_repo(
            "failing_prepare_commit_msg_repo",
            FAILING_PREPARE_CONFIG,
            "failing_prepare_commit_msg_repo",
            None,
        ),
        Fixture::PushExample => {
            let src = cx.git_dir("push_example_src");
            cx.commit(&src, "commit!");
            let clone = env.fresh_dir("push_example_clone");
            cx.git(
                &clone,
                &[
                    OsStr::new("clone"),
                    OsStr::new("-q"),
                    src.as_os_str(),
                    OsStr::new("."),
                ],
            );
            cx.commit(&clone, "commit!");
            clone
        }
        Fixture::OutOfDate { tagged } => {
            let path = cx.make_repo("python_hooks_repo");
            cx.commit(&path, "commit!");
            if tagged {
                cx.git(&path, &["tag", "v1.2.3"]);
            }
            path
        }
        Fixture::HookDisappearing => {
            let path = cx.make_repo("python_hooks_repo");
            write(
                &path.join(".pre-commit-hooks.yaml"),
                HOOK_DISAPPEARING_MANIFEST,
            );
            cx.commit(&path, "modify_manifest");
            path
        }
        Fixture::ImgStaged => {
            let path = cx.git_dir("img_staged");
            std::fs::copy(resources().join("img1.jpg"), path.join("img.jpg"))
                .or_fail("copying img1.jpg");
            cx.git(&path, &["add", "img.jpg"]);
            path
        }
        Fixture::SubStaged => {
            let commits = cx.git_dir("repo_with_commits");
            write(&commits.join("foo"), b"");
            cx.git(&commits, &["add", "foo"]);
            cx.commit(&commits, "commit!");
            let rev1 = cx.head(&commits);
            cx.commit(&commits, "commit!");
            let path = cx.git_dir("sub_staged");
            write(&path.join("bar"), b"");
            cx.git(&path, &["add", "bar"]);
            cx.commit(&path, "commit!");
            cx.git(
                &path,
                &[
                    OsStr::new("submodule"),
                    OsStr::new("add"),
                    OsStr::new("-q"),
                    commits.as_os_str(),
                    OsStr::new("sub"),
                ],
            );
            cx.git(&path.join("sub"), &["checkout", "-q", rev1.as_str()]);
            cx.git(&path, &["add", "sub"]);
            path
        }
    }
}

/// Every resource repository exists on disk (guards the verbatim copy).
#[cfg(test)]
fn missing_resources() -> Vec<&'static str> {
    RESOURCES
        .iter()
        .copied()
        .filter(|r| !resources().join(r).exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_like_pyyaml() {
        assert_eq!(yaml_scalar("bash_hook"), "bash_hook");
        assert_eq!(yaml_scalar("logfile test hook"), "logfile test hook");
        assert_eq!(yaml_scalar("file:///tmp/a b/x"), "file:///tmp/a b/x");
        assert_eq!(yaml_scalar("a: b"), "'a: b'");
        assert_eq!(yaml_scalar("it's #x"), "'it''s #x'");
        assert_eq!(yaml_scalar("-x"), "'-x'");
    }

    #[test]
    fn resources_are_present() {
        assert_eq!(missing_resources(), Vec::<&str>::new());
        assert!(is_script(Path::new("bin/hook.sh")));
        assert!(is_script(Path::new("tty-check-entry")));
        assert!(!is_script(Path::new(".pre-commit-hooks.yaml")));
    }
}
