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
                "command: ('{exe}', 'write-tree')\nreturn code: 128\nstdout: (none)\nstderr:\n    fatal: unmerged"
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
    assert_eq!(
        exc(&e),
        ("FatalError".to_owned(), format!("pre-commit {want_tail}"))
    );
    assert_eq!(e.exit_code(), 1);
    let e = git
        .with_brand(Brand::Snp)
        .diff_index_patch(&tree, &mut Vec::new())
        .unwrap_err();
    assert_eq!(exc(&e).1, format!("snp {want_tail}"));

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
        "command: ('{exe}', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '{}')",
        patch.display()
    )));
    scripted.on("*apply *", exit(0, b"", b""));
    assert!(git.apply(&patch, true, Group::Isolated).unwrap());

    scripted.on(
        "-c submodule.recurse=0 checkout -- .",
        exit(1, b"", b"error: x\n"),
    );
    assert_eq!(
        exc(&git.checkout_all(true, Group::Isolated).unwrap_err()).0,
        "CalledProcessError"
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
        "-c submodule.recurse=0 checkout -- .",
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
    // git's stderr (inherited by Python) reaches stderr through the exception's hint
    assert_eq!(
        exc.hint.as_deref(),
        Some("fatal: bad object 1111111111111111111111111111111111111111")
    );
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
