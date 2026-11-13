//! `ScriptedBackend` (feature `testing`) fails any call by argv glob and records every call; the failure texts of
//! the user-repo calls (13 X.2.2 formats A/B, 02 §7.2).

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_git::testing::{Response, ScriptedBackend};
use snp_git::{DiffIndex, Git};
use snp_proc::spawn::Group;

use crate::common::{Sandbox, exc};

fn exit(code: i64, stdout: &[u8], stderr: &[u8]) -> Response {
    Response::Exit {
        code,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    }
}

fn setup(sb: &Sandbox) -> (std::path::PathBuf, Arc<ScriptedBackend>, Git) {
    let repo = sb.init("repo");
    sb.write(&repo, "a", b"a\n");
    sb.git(&repo, &["add", "a"]);
    sb.commit(&repo, "base");
    let scripted = Arc::new(ScriptedBackend::new());
    let (git, _) = Git::discover(
        sb.exe(),
        &sb.env,
        &repo,
        Arc::<ScriptedBackend>::clone(&scripted),
    )
    .unwrap();
    (repo, scripted, git)
}

fn args(call: &[OsString]) -> String {
    call[1..]
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn every_call_can_fail_and_is_recorded() {
    let sb = Sandbox::new();
    let (repo, scripted, git) = setup(&sb);
    let exe = git_exe_path(&scripted);

    scripted.on("write-tree", exit(128, b"", b"fatal: unmerged\n"));
    let e = git.write_tree().unwrap_err();
    assert_eq!(
        exc(&e),
        (
            "CalledProcessError".to_owned(),
            format!(
                "command: ({}, 'write-tree')\nreturn code: 128\nstdout: (none)\nstderr:\n    fatal: unmerged",
                snp_pycompat::repr::repr_str(&exe)
            )
        )
    );
    assert_eq!(e.exit_code(), 3);

    let tree = snp_git::ObjectId::parse(&crate::common::rep('a', 40)).unwrap();
    scripted.on("diff-index *", exit(2, b"", b"error: permission denied\n"));
    let e = git.diff_index_patch(&tree, &mut Vec::new()).unwrap_err();
    let want_tail = format!(
        "failed to diff -- perhaps due to permissions?\n\ncommand: ('git', 'diff-index', '--ignore-submodules', \
         '--binary', '--exit-code', '--no-color', '--no-ext-diff', '{tree}', '--')\nreturn code: 2\nstdout: \
         (none)\nstderr:\n    error: permission denied"
    );
    // `discover` takes the brand of the process (argv[0] of this test binary is not `pre-commit`): snp
    assert_eq!(
        exc(&e),
        ("FatalError".to_owned(), format!("snp {want_tail}"))
    );
    assert_eq!(e.exit_code(), 1);
    for (brand, prog) in [(Brand::PreCommit, "pre-commit"), (Brand::Snp, "snp")] {
        let e = git
            .with_brand(brand)
            .diff_index_patch(&tree, &mut Vec::new())
            .unwrap_err();
        assert_eq!(exc(&e).1, format!("{prog} {want_tail}"));
    }

    scripted.on("diff-index *", exit(1, b" \n\t", b""));
    assert!(matches!(
        git.diff_index_patch(&tree, &mut Vec::new()).unwrap(),
        DiffIndex::CrlfQuirk
    ));
    scripted.on("diff-index *", exit(1, b"diff --git a/a b/a\n", b""));
    let mut sink = Vec::new();
    assert!(matches!(
        git.diff_index_patch(&tree, &mut sink).unwrap(),
        DiffIndex::Patch { bytes_written: 19 }
    ));
    assert_eq!(sink, b"diff --git a/a b/a\n");
    scripted.on("diff-index *", exit(0, b"", b""));
    assert!(matches!(
        git.diff_index_patch(&tree, &mut Vec::new()).unwrap(),
        DiffIndex::Clean
    ));

    // the two apply variants: the plain one is caught, the autocrlf one raises
    let patch = repo.join("p");
    scripted.on("apply *", exit(1, b"", b"error: patch failed\n"));
    scripted.on(
        "-c core.autocrlf=false apply *",
        exit(1, b"", b"error: a: patch does not apply\n"),
    );
    assert!(!git.apply(&patch, false, Group::Isolated).unwrap());
    let e = git.apply(&patch, true, Group::Isolated).unwrap_err();
    assert!(exc(&e).1.starts_with(&format!(
        "command: ({}, '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', {})",
        snp_pycompat::repr::repr_str(&exe),
        snp_pycompat::repr::repr_str(&patch.display().to_string())
    )));
    scripted.on("*apply *", exit(0, b"", b""));
    assert!(git.apply(&patch, true, Group::Isolated).unwrap());

    // D-DATA-19: the patch's paths come from `git apply --numstat -z` (a binary file's counts are `-`; a name may
    // hold a tab), and the checkout reads them as literal pathspecs from stdin
    scripted.on(
        "apply --numstat -z *",
        exit(0, b"1\t1\tx\0-\t-\tb\tin\0".as_slice(), b""),
    );
    let touched = git.patch_paths(&patch).unwrap();
    assert_eq!(
        touched.iter().collect::<Vec<_>>(),
        [b"x".as_slice(), b"b\tin".as_slice()]
    );
    scripted.on(
        "apply --numstat -z *",
        exit(0, b"1\t1\t\0old\0new\0".as_slice(), b""),
    );
    assert_eq!(
        git.patch_paths(&patch).unwrap().iter().collect::<Vec<_>>(),
        [b"old".as_slice(), b"new".as_slice()]
    );
    scripted.on("apply --numstat -z *", exit(0, b"garbage\0", b""));
    assert_eq!(exc(&git.patch_paths(&patch).unwrap_err()).0, "ValueError");
    scripted.on("apply --numstat -z *", exit(128, b"", b"fatal: x\n"));
    assert_eq!(
        exc(&git.patch_paths(&patch).unwrap_err()).0,
        "CalledProcessError"
    );
    scripted.on(
        "-c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul",
        exit(1, b"", b"error: x\n"),
    );
    let e = git
        .checkout_paths(&touched, true, Group::Isolated)
        .unwrap_err();
    assert_eq!(
        exc(&e),
        (
            "CalledProcessError".to_owned(),
            format!(
                "command: ({}, '-c', 'submodule.recurse=0', 'checkout', '--pathspec-from-file=-', \
                 '--pathspec-file-nul')\nreturn code: 1\nstdout: (none)\nstderr:\n    error: x",
                snp_pycompat::repr::repr_str(&git_exe_path(&scripted))
            )
        )
    );
    // an empty list runs nothing
    let before = scripted.calls().len();
    git.checkout_paths(&snp_git::ZList::default(), true, Group::Isolated)
        .unwrap();
    assert_eq!(scripted.calls().len(), before);
    assert_eq!(
        snp_git::literal_pathspecs(touched.iter()),
        b":(literal)x\0:(literal)b\tin\0"
    );
    // the pathspec switches never reach the checkout; the post-checkout skip does
    let env = git
        .with_env(
            git.env().patched(
                &snp_core::env::EnvPatch::default()
                    .set("GIT_LITERAL_PATHSPECS", "1")
                    .set("GIT_ICASE_PATHSPECS", "1")
                    .set("GIT_GLOB_PATHSPECS", "1")
                    .set("GIT_NOGLOB_PATHSPECS", "1"),
            ),
        )
        .checkout_env(true);
    for v in [
        "GIT_LITERAL_PATHSPECS",
        "GIT_ICASE_PATHSPECS",
        "GIT_GLOB_PATHSPECS",
        "GIT_NOGLOB_PATHSPECS",
    ] {
        assert_eq!(env.get(v), None, "{v}");
    }
    assert_eq!(
        env.get("_PRE_COMMIT_SKIP_POST_CHECKOUT"),
        Some(OsStr::new("1"))
    );
    assert_eq!(
        git.checkout_env(false)
            .get("_PRE_COMMIT_SKIP_POST_CHECKOUT"),
        None
    );
    // the rollback list: the patch's paths first, then every other changed path, each once
    scripted.on(
        "apply --numstat -z *",
        exit(0, b"1\t1\tb\x001\t1\ta\0".as_slice(), b""),
    );
    scripted.on("diff --name-only -z *", exit(0, b"c\0a\0d\0", b""));
    assert_eq!(
        git.rollback_paths(&patch)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [b"b".as_slice(), b"a", b"c", b"d"]
    );

    // xargs callers ignore the return code (rm --cached, add -N)
    let ita = {
        scripted.on("diff --no-renames *", exit(0, b"x\0y\0", b""));
        git.intent_to_add().unwrap()
    };
    scripted.on("rm --cached -- *", exit(1, b"", b"fatal: pathspec\n"));
    git.rm_cached(&ita, Group::Isolated).unwrap();
    scripted.on("add --intent-to-add -- *", Response::SpawnError(2));
    git.add_intent_to_add(&ita, Group::Isolated).unwrap();

    // config_get: rc 1 = unset, others raise
    scripted.on("config --get *", exit(1, b"", b""));
    assert_eq!(git.config_get("core.x").unwrap(), None);
    scripted.on("config --get *", exit(3, b"", b"error: invalid config\n"));
    assert!(git.config_get("core.x").is_err());

    // a spawn failure of a raw call is Python's OSError for 'git'
    scripted.on("status", Response::SpawnError(2));
    let e = git.run_raw(&[OsStr::new("status")]).unwrap_err();
    if cfg!(unix) {
        assert_eq!(
            exc(&e),
            (
                "FileNotFoundError".to_owned(),
                "[Errno 2] No such file or directory: 'git'".to_owned()
            )
        );
    }

    let calls: Vec<String> = scripted.calls().iter().map(|c| args(c)).collect();
    for want in [
        "write-tree",
        "-c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul",
        "rm --cached -- x y",
        "add --intent-to-add -- x y",
        "config --get core.x",
        "status",
    ] {
        assert!(calls.iter().any(|c| c == want), "{want} not in {calls:?}");
    }
    // every call ran in the root with the resolved git first
    assert!(
        scripted
            .calls()
            .iter()
            .all(|c| c[0] == OsString::from(&exe))
    );
}

fn git_exe_path(scripted: &ScriptedBackend) -> String {
    scripted.calls()[0][0].to_string_lossy().into_owned()
}

#[test]
fn a_scripted_panic_reaches_the_caller() {
    let sb = Sandbox::new();
    let (_, scripted, git) = setup(&sb);
    scripted.on("write-tree", Response::Panic);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| git.write_tree()));
    assert!(r.is_err());
}

#[test]
fn pre_push_errors_use_the_stdlib_format_with_a_bare_git() {
    let sb = Sandbox::new();
    let (_, scripted, git) = setup(&sb);
    let sha = crate::common::rep('1', 40);
    scripted.on("cat-file -e *", exit(1, b"", b""));
    scripted.on(
        "rev-list *",
        exit(
            128,
            b"",
            b"fatal: bad object 1111111111111111111111111111111111111111\n",
        ),
    );
    let stdin = format!(
        "refs/heads/a {sha} refs/heads/a {}\n",
        crate::common::rep('0', 40)
    );
    let e = git
        .pre_push(stdin.as_bytes(), OsStr::new("origin"))
        .unwrap_err();
    let snp_core::failure::Failure::Exc { exc, .. } = &e else {
        panic!()
    };
    assert_eq!(
        String::from_utf8_lossy(&exc.text),
        format!(
            "Command '('git', 'rev-list', '{sha}', '--topo-order', '--reverse', '--not', '--remotes=origin')' \
             returned non-zero exit status 128."
        )
    );
    // git's stderr (inherited by Python) goes to the process stderr, never into the exception's K+ hint slot
    assert_eq!(exc.hint, None);
    assert_eq!(e.exit_code(), 3);

    // a 64-hex all-zero local OID is a deletion (D-CRASH-10): skipped without a git call
    let before = scripted.calls().len();
    let z64 = crate::common::rep('0', 64);
    assert_eq!(
        git.pre_push(
            format!("(delete) {z64} refs/heads/gone {z64}\n").as_bytes(),
            OsStr::new("origin")
        )
        .unwrap(),
        None
    );
    assert_eq!(scripted.calls().len(), before);

    // a remote OID present locally: from/to refs without rev-list
    scripted.on("cat-file -e *", exit(0, b"", b""));
    let ns = git
        .pre_push(
            format!("HEAD {sha} refs/heads/a {}\n", crate::common::rep('2', 40)).as_bytes(),
            OsStr::new("origin"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        ns.from_ref,
        Some(OsString::from(crate::common::rep('2', 40)))
    );
    assert_eq!(ns.to_ref, Some(OsString::from(&sha)));
    assert_eq!(
        (ns.local_branch, ns.remote_branch),
        (OsString::from("HEAD"), OsString::from("refs/heads/a"))
    );
    let last = scripted.calls().last().unwrap().clone();
    assert_eq!(
        args(&last),
        format!("cat-file -e {}^{{commit}}", crate::common::rep('2', 40))
    );
}

/// Exact argv is part of the parity contract (it is echoed in `CalledProcessError` texts): every public `Git` call
/// once, compared with pre-commit's argv (`git.py`, `staged_files_only.py`, `run.py`, `hook_impl.py`) plus only the
/// 02 G.7 additions (`--no-renames` on the intent-to-add and staged queries) and the snp-only calls of ARCH §1.4.13.
#[test]
fn every_user_repo_call_has_pre_commits_exact_argv() {
    let sb = Sandbox::new();
    let (repo, scripted, git) = setup(&sb);
    let (_, info) = sb.discover(&repo);
    std::fs::write(
        info.git_dir.join("MERGE_MSG"),
        b"Merge\n\n# Conflicts:\n#\ta\n",
    )
    .unwrap();
    let tree = crate::common::rep('a', 40);
    let local = crate::common::rep('1', 40);
    let first = crate::common::rep('3', 40);
    scripted.on("*", exit(0, b"", b""));
    scripted.on("write-tree", exit(0, format!("{tree}\n").as_bytes(), b""));
    scripted.on("diff --no-renames *", exit(0, b"a\0", b""));
    scripted.on("cat-file -e *", exit(1, b"", b""));
    scripted.on(
        "rev-list * --topo-order *",
        exit(0, format!("{first}\n").as_bytes(), b""),
    );
    let start = scripted.calls().len();

    let ita = git.intent_to_add().unwrap();
    git.has_unmerged().unwrap();
    git.config_is_unstaged(OsStr::new(".pre-commit-config.yaml"))
        .unwrap();
    git.rm_cached(&ita, Group::Inherit).unwrap();
    git.add_intent_to_add(&ita, Group::Inherit).unwrap();
    let oid = git.write_tree().unwrap();
    git.diff_index_patch(&oid, &mut Vec::new()).unwrap();
    let listed = git.patch_paths(std::path::Path::new("p.patch")).unwrap();
    assert!(listed.is_empty());
    git.changed_paths().unwrap();
    git.checkout_paths(&ita, true, Group::Inherit).unwrap();
    git.apply(std::path::Path::new("p.patch"), false, Group::Inherit)
        .unwrap();
    git.apply(std::path::Path::new("p.patch"), true, Group::Inherit)
        .unwrap();
    git.apply_check(std::path::Path::new("p.patch")).unwrap();
    git.add_literal(
        &[snp_core::path::RepoPath::new(b"a").unwrap()],
        Group::Inherit,
    )
    .unwrap();
    git.staged_files().unwrap();
    git.all_files().unwrap();
    git.tracked_with_modes().unwrap();
    git.staged_with_modes().unwrap();
    git.changed_files(OsStr::new("origin/main"), OsStr::new("HEAD"))
        .unwrap();
    git.conflicted_files(&info).unwrap();
    git.diff_fingerprint().unwrap();
    git.show_diff(true).unwrap();
    git.show_diff(false).unwrap();
    git.hooks_path().unwrap();
    git.config_origins().unwrap();
    git.config_get("core.hooksPath").unwrap();
    git.config_get_regexp("^hook\\.").unwrap();
    git.pre_push(
        format!(
            "refs/heads/a {local} refs/heads/a {}\n",
            crate::common::rep('2', 40)
        )
        .as_bytes(),
        OsStr::new("origin"),
    )
    .unwrap();
    git.run_raw(&[OsStr::new("ls-files"), OsStr::new("--")])
        .unwrap();

    let all_files = if sb.exe().caps.ls_files_dedup {
        "ls-files -z --deduplicate"
    } else {
        "ls-files -z"
    };
    let golden: Vec<String> = [
        "diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z",
        "ls-files --unmerged",
        "diff --quiet --no-ext-diff .pre-commit-config.yaml",
        "rm --cached -- a",
        "add --intent-to-add -- a",
        "write-tree",
        &format!(
            "diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff {tree} --"
        ),
        "apply --numstat -z --whitespace=nowarn p.patch",
        "diff --name-only -z --no-renames --no-ext-diff --no-color --ignore-submodules --no-relative",
        "-c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul",
        "apply --whitespace=nowarn p.patch",
        "-c core.autocrlf=false apply --whitespace=nowarn p.patch",
        "apply --check --whitespace=nowarn p.patch",
        "--literal-pathspecs add -- a",
        "diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB --no-renames",
        all_files,
        "ls-files -s -z",
        "diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB",
        "diff --name-only --no-ext-diff -z origin/main...HEAD",
        "write-tree",
        &format!("diff --name-only --no-ext-diff -z -m {tree} HEAD MERGE_HEAD --"),
        "diff --no-ext-diff --no-textconv --ignore-submodules",
        "--no-pager diff --no-ext-diff --color=always",
        "--no-pager diff --no-ext-diff --color=never",
        "config core.hooksPath",
        "config --list --show-origin -z",
        "config --get core.hooksPath",
        "config -z --get-regexp ^hook\\.",
        &format!("cat-file -e {}^{{commit}}", crate::common::rep('2', 40)),
        &format!("rev-list {local} --topo-order --reverse --not --remotes=origin"),
        &format!("rev-list --max-parents=0 {local}"),
        &format!("rev-parse {first}^"),
        "ls-files --",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let calls = scripted.calls();
    let got: Vec<String> = calls[start..].iter().map(|c| args(c)).collect();
    assert_eq!(got, golden);
    // argv[0] is always the resolved git, never a bare name
    let exe = git_exe_path(&scripted);
    assert!(calls[start..].iter().all(|c| c[0] == OsStr::new(&exe)));
}

/// `subprocess.run(encoding='utf-8', check=True)` decodes stdout before checking the returncode.
#[test]
fn lfs_files_decodes_before_checking_the_returncode() {
    let sb = Sandbox::new();
    let (_, scripted, git) = setup(&sb);
    let facts = snp_git::pch_facts::PchFacts::new(git);
    scripted.on("check-attr *", exit(1, b"a\0filter\0\xff\0", b""));
    let e = facts.lfs_files(&[b"a".as_slice()]).unwrap_err();
    assert_eq!(exc(&e).0, "UnicodeDecodeError");
    scripted.on("check-attr *", exit(1, b"a\0filter\0lfs\0", b""));
    let e = facts.lfs_files(&[b"a".as_slice()]).unwrap_err();
    assert_eq!(
        exc(&e),
        (
            "CalledProcessError".to_owned(),
            "Command '('git', 'check-attr', 'filter', '-z', '--stdin')' returned non-zero exit status 1."
                .to_owned()
        )
    );
}
