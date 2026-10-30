//! The in-process half of the fault-injection matrix (ARCHITECTURE §4.7, FI-01…FI-30): git failures through
//! `snp_git::testing::ScriptedBackend`, panics injected through `snp_core::fault` after every transition and inside
//! the Printer lock. The process-level rows (signals, kill -9, full disk) are in `tests/driver.rs`.
//!
//! After every case the invariant of §4.7 is checked: worktree + index (+ patch + journal + pointer when the restore
//! did not complete) reconstruct the pre-run state, and the recovery notice names a working command.
//!
//! Run with `cargo test -p snp-stash --features fault-injection --test fault`.
//!
//! Owner: WP-23.

#[cfg(test)]
#[expect(
    dead_code,
    reason = "shared with the catalog binary; this binary uses a subset"
)]
mod common;

#[cfg(test)]
mod fi {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard};

    use snp_core::env::Env;
    use snp_core::failure::Failure;
    use snp_core::output::Level;
    use snp_core::path::RepoPath;
    use snp_git::GitBackend;
    use snp_git::testing::{Response, ScriptedBackend};

    use crate::common::{FOO, Sandbox, exc};

    /// Fault points are process-global: one case at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn arm(spec: &str) {
        snp_core::fault::arm_from_env(&Env::from_pairs([("SNP_FAULT", spec)]));
    }

    fn disarm() {
        snp_core::fault::arm_from_env(&Env::from_pairs([("X", "")]));
    }

    /// A repository with a staged `foo`, an unstaged change to it, a staged `fixme` and an intent-to-add `n`.
    struct Case {
        sb: Sandbox,
        repo: PathBuf,
        diff: Vec<u8>,
        /// The patch snp saves: the unstaged diff without the intent-to-add file.
        patch_diff: Vec<u8>,
        index: String,
        tree: String,
    }

    impl Case {
        fn new() -> Self {
            let sb = Sandbox::new();
            let repo = sb.init("r");
            sb.write(&repo, "foo", FOO.as_bytes());
            sb.write(&repo, "fixme", b"x  \n");
            sb.write(&repo, "n", b"new\n");
            sb.git(&repo, &["add", "foo", "fixme"]);
            sb.git(&repo, &["add", "--intent-to-add", "n"]);
            sb.write(&repo, "foo", FOO.replace('3', "three").as_bytes());
            let diff = sb.unstaged_diff(&repo);
            let patch_diff = sb.git(
                &repo,
                &[
                    "diff",
                    "--binary",
                    "--no-ext-diff",
                    "--no-color",
                    "--",
                    "foo",
                ],
            );
            let index = sb.index(&repo);
            let tree = {
                // The tree snp records: the index without the intent-to-add entry.
                let ita_free = sb.path("ita-free-index");
                std::fs::copy(repo.join(".git/index"), &ita_free).unwrap();
                let out = std::process::Command::new(
                    snp_proc::resolve::find_executable(OsStr::new("git"), &sb.env, &repo).unwrap(),
                )
                .args(["rm", "-q", "--cached", "--", "n"])
                .current_dir(&repo)
                .env_clear()
                .envs(sb.env.iter())
                .env("GIT_INDEX_FILE", &ita_free)
                .output()
                .unwrap();
                assert!(out.status.success());
                let out = std::process::Command::new(
                    snp_proc::resolve::find_executable(OsStr::new("git"), &sb.env, &repo).unwrap(),
                )
                .arg("write-tree")
                .current_dir(&repo)
                .env_clear()
                .envs(sb.env.iter())
                .env("GIT_INDEX_FILE", &ita_free)
                .output()
                .unwrap();
                String::from_utf8(out.stdout).unwrap().trim().to_owned()
            };
            Self {
                sb,
                repo,
                diff,
                patch_diff,
                index,
                tree,
            }
        }

        fn scripted(
            &self,
        ) -> (
            Arc<ScriptedBackend>,
            snp_stash::StashCtx,
            crate::common::Buf,
            crate::common::Buf,
        ) {
            let b = Arc::new(ScriptedBackend::new());
            let backend: Arc<dyn GitBackend> = Arc::<ScriptedBackend>::clone(&b);
            let (git, info) = self.sb.discover(&self.repo);
            let (cx, out, err) = self.sb.ctx_with(git.with_backend(backend), info);
            (b, cx, out, err)
        }

        /// The pre-run state exactly (the restore completed).
        fn assert_pre_run_state(&self) {
            assert_eq!(self.sb.unstaged_diff(&self.repo), self.diff, "worktree");
            assert_eq!(self.sb.index(&self.repo), self.index, "index");
        }

        /// The restore did not complete: the recovery notice names the patch, which reconstructs the unstaged diff.
        fn assert_recoverable(&self) {
            let found =
                snp_stash::find_unrestored(&self.repo.join(".git"), &self.sb.patches(), &self.repo);
            assert_eq!(found.len(), 1, "one orphan");
            let u = &found[0];
            assert_eq!(u.tree.as_deref(), Some(self.tree.as_str()));
            assert!(
                u.hint()
                    .ends_with(&format!("git apply --3way {}", u.patch.display()))
            );
            assert_eq!(
                std::fs::read(&u.patch).unwrap(),
                self.patch_diff,
                "the patch is the unstaged diff"
            );
        }

        fn ok() -> Response {
            Response::Exit {
                code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }
        }
    }

    fn fail(code: i64, stderr: &str) -> Response {
        Response::Exit {
            code,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn rp(s: &str) -> RepoPath {
        RepoPath::new(s.as_bytes()).unwrap()
    }

    #[test]
    fn fi_01_write_tree_fails() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        b.on("write-tree", fail(128, "fatal: x\n"));
        let e = snp_stash::enter(cx).unwrap_err();
        assert_eq!(exc(&e).0, "CalledProcessError");
        c.assert_pre_run_state();
    }

    #[test]
    fn fi_02_diff_index_rc_2() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        b.on("diff-index *", fail(2, "error: x\n"));
        let e = snp_stash::enter(cx).unwrap_err();
        assert_eq!((exc(&e).0.as_str(), e.exit_code()), ("FatalError", 1));
        c.assert_pre_run_state();
        assert!(c.sb.patch_files().iter().all(|f| !f.starts_with("patch")));
    }

    #[test]
    fn fi_03_checkout_fails() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, out, _) = c.scripted();
        // Only the stash checkout fails; the rollback checkout is real.
        b.on(
            "-c submodule.recurse=0 checkout -- .",
            fail(1, "error: x\n"),
        );
        // The scripted rule also fails the rollback checkout: the K+ path keeps the patch.
        let e = snp_stash::enter(cx).unwrap_err();
        assert_eq!(exc(&e).0, "CalledProcessError");
        assert!(out.text().contains("Rolling back fixes..."));
        // The worktree was never cleaned (the checkout failed), so the user's change is still there.
        assert_eq!(c.sb.unstaged_diff(&c.repo), c.diff);
        assert_eq!(c.sb.status_of(&c.repo, "n"), "A");
    }

    #[test]
    fn fi_04_first_apply_fails_autocrlf_apply_succeeds() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, out, _) = c.scripted();
        b.on("apply --whitespace=nowarn *", fail(1, "error: x\n"));
        let g = snp_stash::enter(cx).unwrap();
        g.restore().unwrap();
        assert!(!out.text().contains("Rolling back"));
        c.assert_pre_run_state();
        assert!(
            b.calls()
                .iter()
                .any(|a| a.iter().any(|x| x == "core.autocrlf=false"))
        );
    }

    #[test]
    fn fi_05_both_applies_fail_then_rollback_succeeds() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, out, _) = c.scripted();
        let g = snp_stash::enter(cx).unwrap();
        // A conflicting hook fix: both applies fail for real, rollback, apply.
        c.sb.write(&c.repo, "foo", FOO.replace('3', "drei").as_bytes());
        g.restore().unwrap();
        assert!(out.text().contains("Rolling back fixes..."));
        c.assert_pre_run_state();
        let applies = b
            .calls()
            .iter()
            .filter(|a| a.iter().any(|x| x == "apply"))
            .count();
        assert_eq!(applies, 3);
    }

    #[test]
    fn fi_06_rollback_checkout_fails() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, out, _) = c.scripted();
        let g = snp_stash::enter(cx).unwrap();
        b.on("apply *", fail(1, ""))
            .on("-c core.autocrlf=false apply *", fail(1, ""));
        b.on(
            "-c submodule.recurse=0 checkout -- .",
            fail(1, "error: rollback\n"),
        );
        let e = g.restore().unwrap_err();
        let (ty, text, hint) = exc(&e);
        assert_eq!(ty, "CalledProcessError");
        assert!(text.contains("'checkout', '--', '.'"));
        assert!(
            hint.unwrap()
                .starts_with("[ERROR] Your unstaged changes are saved in ")
        );
        assert!(!out.text().contains("Restored changes"));
        c.assert_recoverable();
        assert_eq!(
            c.sb.status_of(&c.repo, "n"),
            "A",
            "the intent-to-add entry is re-added anyway"
        );
    }

    #[test]
    fn fi_07_applies_fail_after_the_rollback() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        let g = snp_stash::enter(cx).unwrap();
        b.on("apply *", fail(1, ""))
            .on("-c core.autocrlf=false apply *", fail(1, "error: final\n"));
        let e = g.restore().unwrap_err();
        assert!(exc(&e).1.contains("error: final"));
        c.assert_recoverable();
    }

    #[test]
    fn fi_08_rm_cached_fails_rc_ignored() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, out, _) = c.scripted();
        b.on("rm --cached -- *", fail(1, "fatal: x\n"));
        // pre-commit ignores the returncode: the intent-to-add file then lands in the patch, the checkout leaves it
        // in place and every apply fails with `already exists in working directory` — the K+ path keeps the patch.
        let g = snp_stash::enter(cx).unwrap();
        let e = g.restore().unwrap_err();
        assert!(exc(&e).1.contains("already exists in working directory"));
        assert!(
            out.text()
                .starts_with("[WARNING] Unstaged intent-to-add files detected.\n")
        );
        let found = snp_stash::find_unrestored(&c.repo.join(".git"), &c.sb.patches(), &c.repo);
        let patch = std::fs::read(&found[0].patch).unwrap();
        assert!(String::from_utf8_lossy(&patch).contains("+three"));
        assert!(
            String::from_utf8_lossy(&patch).contains("+new"),
            "the rolled-back intent-to-add content is in the patch"
        );
    }

    #[test]
    fn fi_09_add_intent_to_add_fails_rc_ignored() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        let g = snp_stash::enter(cx).unwrap();
        b.on("add --intent-to-add -- *", fail(1, "fatal: x\n"));
        g.restore().unwrap();
        // pre-commit ignores the returncode: the entry is lost, the content stays in the worktree.
        assert_eq!(c.sb.read(&c.repo, "n"), b"new\n");
        assert_eq!(
            c.sb.read(&c.repo, "foo"),
            FOO.replace('3', "three").as_bytes()
        );
    }

    #[test]
    fn fi_10_stage_fixed_add_fails() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        let mut g = snp_stash::enter(cx).unwrap();
        c.sb.write(&c.repo, "fixme", b"x\n");
        b.on(
            "--literal-pathspecs add -- *",
            fail(128, "fatal: index.lock\n"),
        );
        let e = g.stage_fixed(&[rp("fixme")]).unwrap_err();
        assert_eq!(exc(&e).0, "CalledProcessError");
        // The journal says staging-fixed (written before the add).
        let found = snp_stash::find_unrestored(&c.repo.join(".git"), &c.sb.patches(), &c.repo);
        assert_eq!(&*found[0].state, "staging-fixed");
        assert_eq!(found[0].staged_fixed, vec![rp("fixme")]);
        g.restore().unwrap();
        assert_eq!(
            c.sb.read(&c.repo, "foo"),
            FOO.replace('3', "three").as_bytes()
        );
    }

    #[test]
    fn fi_11_spawn_failure_of_the_apply_is_k_plus() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        let g = snp_stash::enter(cx).unwrap();
        b.on("apply *", Response::SpawnError(24))
            .on("-c core.autocrlf=false apply *", Response::SpawnError(24));
        b.on("-c submodule.recurse=0 checkout -- .", Case::ok());
        let e = g.restore().unwrap_err();
        assert!(exc(&e).2.is_some());
        c.assert_recoverable();
    }

    /// FI-12…FI-21: a panic after each state transition (`StashState(n)`) unwinds through the guard, whose `Drop`
    /// restores the pre-run state.
    #[test]
    fn fi_12_to_21_panic_after_each_transition_restores() {
        let _s = serial();
        for n in 1..=10_u8 {
            let c = Case::new();
            let (_b, cx, _, _) = c.scripted();
            arm(&format!("StashState({n})"));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut g = snp_stash::enter(cx)?;
                c.sb.write(&c.repo, "fixme", b"x\n");
                g.stage_fixed(&[rp("fixme")])?;
                g.restore()
            }));
            disarm();
            // 1–5 fire inside enter, 6/7 inside stage_fixed, 9/10 in the restore; 8 is the rollback (not reached).
            if n == 8 {
                assert!(
                    matches!(r, Ok(Ok(()))),
                    "StashState(8) is not reached without a conflict"
                );
            } else {
                let payload = r.unwrap_err();
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(msg, format!("snp fault injection: StashState({n})"));
            }
            // The worktree always gets the user's change back, and the intent-to-add entry.
            assert_eq!(
                c.sb.read(&c.repo, "foo"),
                FOO.replace('3', "three").as_bytes(),
                "StashState({n})"
            );
            assert_eq!(c.sb.status_of(&c.repo, "n"), "A", "StashState({n})");
            match n {
                1..=5 => c.assert_pre_run_state(),
                // The panic hit before the add: nothing staged.
                6 => assert_eq!(c.sb.git(&c.repo, &["show", ":fixme"]), b"x  \n"),
                // After the add (or later): the fix is staged, as the run intended.
                _ => assert_eq!(
                    c.sb.git(&c.repo, &["show", ":fixme"]),
                    b"x\n",
                    "StashState({n})"
                ),
            }
            assert!(
                snp_stash::find_unrestored(&c.repo.join(".git"), &c.sb.patches(), &c.repo)
                    .is_empty()
            );
            assert!(
                !c.repo.join(".git/snp-stash.journal").exists(),
                "StashState({n})"
            );
        }
    }

    /// FI-22…FI-26: a panic before each restore command (`RestoreCommand(n)`: plain apply, autocrlf apply, rollback
    /// checkout, plain apply, autocrlf apply, intent-to-add re-add); the `Drop` fallback resumes and finishes it.
    #[test]
    fn fi_22_to_26_panic_before_each_restore_command() {
        let _s = serial();
        for n in 1..=6_u8 {
            let c = Case::new();
            let (b, cx, out, _) = c.scripted();
            let g = snp_stash::enter(cx).unwrap();
            // Every command runs: the plain applies fail (scripted), the hook fix conflicts until the rollback.
            b.on("apply --whitespace=nowarn *", fail(1, ""));
            c.sb.write(&c.repo, "foo", FOO.replace('3', "drei").as_bytes());
            arm(&format!("RestoreCommand({n})"));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || g.restore()));
            disarm();
            let payload = r.unwrap_err();
            assert_eq!(
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default(),
                format!("snp fault injection: RestoreCommand({n})")
            );
            c.assert_pre_run_state();
            assert!(
                out.text().contains("[INFO] Restored changes from "),
                "RestoreCommand({n})"
            );
            assert!(
                snp_stash::find_unrestored(&c.repo.join(".git"), &c.sb.patches(), &c.repo)
                    .is_empty()
            );
        }
    }

    #[test]
    fn fi_27_panic_in_a_worker_holding_the_printer_lock() {
        let _s = serial();
        let c = Case::new();
        let (_b, cx, out, _) = c.scripted();
        let printer = Arc::clone(&cx.printer);
        let g = snp_stash::enter(cx).unwrap();
        arm("PrinterWrite");
        let worker = std::thread::Builder::new()
            .spawn(move || printer.log(Level::Info, "from a worker"))
            .unwrap();
        let joined = worker.join();
        disarm();
        let failure = match joined {
            Err(p) => Failure::Exc {
                kind: snp_core::failure::ExcKind::Unexpected,
                exc: snp_core::failure::PyExc::panic(
                    p.downcast_ref::<String>().map_or("?", String::as_str),
                    None,
                    None,
                ),
            },
            Ok(()) => panic!("the fault did not fire"),
        };
        assert_eq!(exc(&failure).1, "snp fault injection: PrinterWrite");
        // The poisoned printer still prints the restore messages; no hang.
        g.restore().unwrap();
        assert!(out.text().contains("[INFO] Restored changes from "));
        c.assert_pre_run_state();
    }

    #[test]
    fn fi_28_scripted_panic_inside_a_git_call_restores() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        let g = snp_stash::enter(cx).unwrap();
        b.on("apply --whitespace=nowarn *", Response::Panic);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || g.restore()));
        assert!(r.is_err());
        // The journal makes it recoverable.
        c.assert_recoverable();
    }

    #[test]
    fn fi_29_panic_in_enter_before_the_stash_leaves_nothing_behind() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        b.on("write-tree", Response::Panic);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            snp_stash::enter(cx).map(drop)
        }));
        assert!(r.is_err());
        c.assert_pre_run_state();
        assert!(
            snp_stash::find_unrestored(&c.repo.join(".git"), &c.sb.patches(), &c.repo).is_empty()
        );
        assert!(!c.repo.join(".git/snp-stash.journal").exists());
    }

    #[test]
    fn fi_30_spawn_errors_before_the_stash_are_oserror() {
        let _s = serial();
        let c = Case::new();
        let (b, cx, _, _) = c.scripted();
        b.on("diff --no-renames *", Response::SpawnError(2));
        let e = snp_stash::enter(cx).unwrap_err();
        assert_eq!(e.exit_code(), 3);
        c.assert_pre_run_state();
        assert!(Path::new(&c.repo).is_dir());
    }
}
