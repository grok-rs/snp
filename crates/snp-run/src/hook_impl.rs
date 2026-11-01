//! `hook-impl` (`commands/hook_impl.py`; §3.10). `hook_impl()` contains no `?` and no early `return` (§4.4,
//! `xtask lint-guards`): its body is [`body`], and the stash of the nested run is restored by the run pipeline's own
//! single exit.
//!
//! Order (pre-commit's): legacy phase (migration-mode guard, pre-push stdin read in full — more than 64 MiB fails
//! closed, D-SEC-12 —, the `--hook-dir` CDPATH repair, D-DATA-07, the `<type>.legacy` hook, a broken one being a
//! failed legacy hook, D-CRASH-14) → the `SNP=0` kill switch (D-EXT-05) → the missing-config texts → the arity
//! check → the unknown-type `AssertionError` → the namespace → `legacy | run`.
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_cli::{HookImplArgs, RunArgs, RunExt};
use snp_core::env::EnvPatch;
use snp_core::failure::Failure;
use snp_core::stage::{ArgCount, HookType, Stage};
use snp_proc::spawn::{self, Group, Role, Spec, Stdin};

use crate::files::STDIN_CAP;
use crate::pipeline::{self, Invocation};
use crate::{RunCtx, common, messages};

/// `pre-commit hook-impl`: the git hook's entry (legacy hook, then `run`).
#[expect(
    clippy::needless_pass_by_value,
    reason = "frozen signature (ARCHITECTURE §1.4.21)"
)]
pub fn hook_impl(a: HookImplArgs, stdin: &mut dyn Read, cx: &RunCtx) -> Result<i32, Failure> {
    let outcome = body(&a, stdin, cx);
    cx.printer.flush();
    outcome
}

/// `hook_impl.hook_impl` after `_adjust_args_and_chdir`.
fn body(a: &HookImplArgs, stdin: &mut dyn Read, cx: &RunCtx) -> Result<i32, Failure> {
    let (git, info) = common::discover(cx)?;
    let root = info.root.clone();
    let config = common::adjust_config(&a.config, &cx.invocation_cwd, &root);
    common::mark_config_used(&cx.store, &root.join(&config), &root);
    let hook_type = a.hook_type.to_string_lossy().into_owned();
    // `main.py:408`: `args.rest[1:]`, whatever the first token is
    let args: Vec<OsString> = a.rest.iter().skip(1).cloned().collect();
    // ---- legacy phase (`_run_legacy`) ----
    if cx.env.get_nonempty("PRE_COMMIT_RUNNING_LEGACY").is_some()
        || cx.env.get_nonempty("SNP_RUNNING_LEGACY").is_some()
    {
        let mut text = messages::migration_mode(cx.brand, &hook_type);
        text.push('\n');
        return Err(Failure::SystemExit {
            code: 1,
            stderr: text.into_bytes(),
        });
    }
    let input: Arc<[u8]> = if hook_type == "pre-push" {
        Arc::from(read_capped(stdin)?)
    } else {
        Arc::from(Vec::new())
    };
    let retv = match a.hook_dir.as_deref().map(repair_hook_dir) {
        None => 0,
        Some(dir) => run_legacy(cx, &root, &dir, &hook_type, &args, &input),
    };
    // ---- kill switch (ADR-038) ----
    if kill_switch(cx) {
        if allow_disable(cx, &root, &config) {
            return Ok(retv);
        }
        cx.printer
            .diag(&messages::kill_switch_refused(&config.to_string_lossy()));
    }
    // ---- `_validate_config` ----
    if !root.join(&config).is_file() {
        let shown = config.to_string_lossy();
        if a.skip_on_missing_config || cx.env.get_nonempty("PRE_COMMIT_ALLOW_NO_CONFIG").is_some() {
            cx.printer
                .py_print(&messages::config_not_found_skipping(cx.brand, &shown));
            return Ok(retv);
        }
        cx.printer
            .py_print(&messages::no_config_file(cx.brand, &shown));
        return Ok(1);
    }
    // ---- `_run_ns` ----
    let Some(ty) = HookType::parse(&hook_type) else {
        return Err(Failure::unexpected(
            "AssertionError",
            messages::unexpected_hook_type(&hook_type),
        ));
    };
    check_args_length(ty, &args)?;
    let mut ns = namespace(ty, a, config);
    if ty == HookType::PrePush {
        let remote_name = args.first().cloned().unwrap_or_default();
        match git.pre_push(&input, &remote_name)? {
            None => return Ok(retv),
            Some(p) => {
                ns.all_files = p.all_files;
                ns.from_ref = p.from_ref;
                ns.to_ref = p.to_ref;
                ns.remote_branch = Some(p.remote_branch);
                ns.local_branch = Some(p.local_branch);
                ns.remote_name = args.first().cloned();
                ns.remote_url = args.get(1).cloned();
            }
        }
    }
    let inv = Invocation {
        store: &cx.store,
        track_config: false,
        direct: false,
        fixed_paths: true,
        hook_stdin: Some(input),
        discovered: Some((git, info)),
    };
    let rc = pipeline::run_pipeline(&ns, cx, &inv)?;
    Ok(retv | rc)
}

/// Reads stdin completely; more than 64 MiB fails closed before the legacy hook (D-SEC-12, ADR-076).
fn read_capped(stdin: &mut dyn Read) -> Result<Vec<u8>, Failure> {
    let mut buf = Vec::new();
    stdin
        .take(STDIN_CAP.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| Failure::from_io(&e, None, None))?;
    if u64::try_from(buf.len()).unwrap_or(u64::MAX) > STDIN_CAP {
        return Err(Failure::fatal("FatalError", messages::STDIN_TOO_LARGE));
    }
    Ok(buf)
}

/// D-DATA-07: an exported `CDPATH` makes the script's `cd "$(dirname "$0")"` print the directory, so `HERE` holds
/// it twice (`dir\ndir`). All lines equal and an existing directory ⇒ that directory; anything else is kept.
pub(crate) fn repair_hook_dir(dir: &OsStr) -> PathBuf {
    let bytes = dir.as_encoded_bytes();
    if bytes.contains(&b'\n') {
        let mut lines = bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(hook_dir_line);
        if let Some(first) = lines.next()
            && lines.all(|l| l == first)
            && first.is_dir()
        {
            return first;
        }
    }
    PathBuf::from(dir)
}

/// One line of a doubled `HERE`. On Windows the script runs in Git for Windows' MSYS2 shell, whose `pwd` prints
/// `/d/a/x` for `D:\a\x`; MSYS2 rewrites such an argument for a native program only when it is a single path, so
/// the lines of the doubled one arrive in the shell's form and are read as `D:/a/x`.
fn hook_dir_line(line: &[u8]) -> PathBuf {
    let native = if cfg!(windows) {
        msys_drive_path(line)
    } else {
        None
    };
    native.map_or_else(
        || PathBuf::from(crate::files::os_from_bytes(line)),
        PathBuf::from,
    )
}

/// `/d` or `/d/rest` (an MSYS2 drive path) as `D:/` or `D:/rest`; `None` for anything else.
fn msys_drive_path(line: &[u8]) -> Option<String> {
    let (drive, rest) = match line {
        [b'/', d] => (*d, &[][..]),
        [b'/', d, b'/', rest @ ..] => (*d, rest),
        _ => return None,
    };
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let rest = std::str::from_utf8(rest).ok()?;
    Some(format!(
        "{}:/{rest}",
        char::from(drive.to_ascii_uppercase())
    ))
}

/// `os.access(legacy_hook, os.X_OK)`: directories pass too (X13-HI-07).
fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.exists()
    }
}

/// Runs `<hook_dir>/<type>.legacy` with inherited stdio; a broken one (missing interpreter, a directory, a spawn
/// error) is a failed legacy hook: its message on stderr, rc 1 (D-CRASH-14).
fn run_legacy(
    cx: &RunCtx,
    root: &Path,
    dir: &Path,
    hook_type: &str,
    args: &[OsString],
    input: &Arc<[u8]>,
) -> i32 {
    let legacy = dir.join(format!("{hook_type}.legacy"));
    if !executable(&legacy) {
        return 0;
    }
    let env = cx.env.patched(
        &EnvPatch::default()
            .set("PRE_COMMIT_RUNNING_LEGACY", "1")
            .set("SNP_RUNNING_LEGACY", "1"),
    );
    let mut cmd = vec![legacy.into_os_string()];
    cmd.extend(args.iter().cloned());
    let argv = match snp_proc::resolve::normalize_cmd(&cmd, &env, root) {
        Ok(v) => v,
        Err(e) => {
            cx.printer.diag(&e.message());
            return 1;
        }
    };
    cx.printer.flush();
    let spec = Spec {
        argv: &argv,
        env: &env,
        cwd: root,
        stdin: Stdin::Bytes(Arc::clone(input)),
        group: Group::Inherit,
        role: Role::Hook,
        deadline: None,
        cancel: &cx.cancel,
    };
    let rc = match spawn::inherit(&spec) {
        Ok(s) => i32::try_from(s.0).unwrap_or(1),
        Err(e) => {
            cx.printer.diag(&e.python_text());
            1
        }
    };
    snp_os::term::ensure_vt();
    rc
}

/// `SNP=0|false` (and not `SNP_COMPAT_STRICT=1`).
fn kill_switch(cx: &RunCtx) -> bool {
    common::ext_mode(&cx.env) != snp_config::ExtMode::Strict
        && cx
            .env
            .get("SNP")
            .and_then(OsStr::to_str)
            .is_some_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}

/// The root `allow_disable`, read leniently: a missing or invalid config counts as `true`.
fn allow_disable(cx: &RunCtx, root: &Path, config: &OsStr) -> bool {
    let sink = snp_core::output::CollectWarnings::new();
    let src = common::config_source(config, root);
    common::load_config_with(cx, &src, &sink, &sink).map_or(true, |c| c.root_ext.allow_disable)
}

/// `_check_args_length`: the `SystemExit` texts with Python's list repr.
fn check_args_length(ty: HookType, args: &[OsString]) -> Result<(), Failure> {
    let n = args.len();
    let (ok, expected) = match ty.arg_count() {
        ArgCount::Exact(k) => (
            n == usize::from(k),
            format!("{k} {}", if k == 1 { "argument" } else { "arguments" }),
        ),
        ArgCount::Range(1, 3) => ((1..=3).contains(&n), String::from("1, 2, or 3 arguments")),
        ArgCount::Range(lo, hi) => (
            (usize::from(lo)..=usize::from(hi)).contains(&n),
            format!("{lo} or {hi} arguments"),
        ),
    };
    if ok {
        return Ok(());
    }
    let repr: Vec<String> = args
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let mut text = messages::arity(
        ty.as_str(),
        &expected,
        n,
        &snp_pycompat::repr::repr_str_list(&repr),
    );
    text.push('\n');
    Err(Failure::SystemExit {
        code: 1,
        stderr: text.into_bytes(),
    })
}

/// `_ns(...)`: the run namespace of a hook type (pre-push's refs are filled by the caller).
fn namespace(ty: HookType, a: &HookImplArgs, config: OsString) -> RunArgs {
    let rest: Vec<OsString> = a.rest.iter().skip(1).cloned().collect();
    let arg = |i: usize| rest.get(i).cloned();
    let mut ns = RunArgs {
        color: a.color,
        config,
        hooks: Vec::new(),
        verbose: false,
        all_files: false,
        files: Vec::new(),
        show_diff_on_failure: false,
        fail_fast: false,
        hook_stage: Stage::Hook(ty),
        remote_branch: None,
        local_branch: None,
        from_ref: None,
        to_ref: None,
        pre_rebase_upstream: None,
        pre_rebase_branch: None,
        commit_msg_filename: None,
        prepare_commit_message_source: None,
        commit_object_name: None,
        remote_name: None,
        remote_url: None,
        checkout_type: None,
        is_squash_merge: None,
        rewrite_command: None,
        ext: RunExt::default(),
    };
    match ty {
        HookType::CommitMsg => ns.commit_msg_filename = arg(0),
        HookType::PrepareCommitMsg => {
            ns.commit_msg_filename = arg(0);
            ns.prepare_commit_message_source = arg(1);
            ns.commit_object_name = arg(2);
        }
        HookType::PostCheckout => {
            ns.from_ref = arg(0);
            ns.to_ref = arg(1);
            ns.checkout_type = arg(2);
        }
        HookType::PostMerge => ns.is_squash_merge = arg(0),
        HookType::PostRewrite => ns.rewrite_command = arg(0),
        HookType::PreRebase => {
            ns.pre_rebase_upstream = arg(0);
            ns.pre_rebase_branch = arg(1);
        }
        _ => {}
    }
    ns
}

#[cfg(test)]
mod tests {
    use super::*;
    use snp_core::output::ColorSetting;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    fn err_text(r: Result<(), Failure>) -> String {
        match r {
            Err(Failure::SystemExit { code: 1, stderr }) => String::from_utf8(stderr).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn t10_hki_03_valid_arg_counts() {
        for (t, n) in [
            ("pre-commit", 0),
            ("pre-merge-commit", 0),
            ("pre-push", 2),
            ("commit-msg", 1),
            ("post-commit", 0),
            ("post-merge", 1),
            ("pre-rebase", 1),
            ("pre-rebase", 2),
            ("post-checkout", 3),
            ("post-rewrite", 1),
            ("prepare-commit-msg", 1),
            ("prepare-commit-msg", 2),
            ("prepare-commit-msg", 3),
        ] {
            let args: Vec<OsString> = (0..n).map(|i| OsString::from(i.to_string())).collect();
            check_args_length(HookType::parse(t).unwrap(), &args).unwrap();
        }
    }

    #[test]
    fn t10_hki_04_wrong_arg_counts() {
        assert_eq!(
            err_text(check_args_length(
                HookType::PreCommit,
                &os(&["run", "--all-files"])
            )),
            "hook-impl for pre-commit expected 0 arguments but got 2: ['run', '--all-files']\n"
        );
        assert_eq!(
            err_text(check_args_length(HookType::CommitMsg, &[])),
            "hook-impl for commit-msg expected 1 argument but got 0: []\n"
        );
        assert_eq!(
            err_text(check_args_length(HookType::PrepareCommitMsg, &[])),
            "hook-impl for prepare-commit-msg expected 1, 2, or 3 arguments but got 0: []\n"
        );
        assert_eq!(
            err_text(check_args_length(HookType::PreRebase, &[])),
            "hook-impl for pre-rebase expected 1 or 2 arguments but got 0: []\n"
        );
        // X13-HI-02
        assert_eq!(
            err_text(check_args_length(HookType::PrePush, &os(&["origin"]))),
            "hook-impl for pre-push expected 2 arguments but got 1: ['origin']\n"
        );
    }

    fn hi(ty: &str, rest: &[&str]) -> HookImplArgs {
        HookImplArgs {
            color: ColorSetting::Always,
            config: OsString::from(".pre-commit-config.yaml"),
            hook_type: OsString::from(ty),
            hook_dir: None,
            skip_on_missing_config: false,
            rest: os(rest),
        }
    }

    #[test]
    fn t10_hki_05_namespace_mapping() {
        let ns = |ty: HookType, rest: &[&str]| {
            namespace(ty, &hi(ty.as_str(), rest), OsString::from("c.yaml"))
        };
        let n = ns(HookType::PreRebase, &["--", "main", "topic"]);
        assert_eq!(n.pre_rebase_upstream.as_deref(), Some(OsStr::new("main")));
        assert_eq!(n.pre_rebase_branch.as_deref(), Some(OsStr::new("topic")));
        let n = ns(HookType::PreRebase, &["--", "main"]);
        assert_eq!(n.pre_rebase_branch, None);
        let n = ns(HookType::CommitMsg, &["--", ".git/COMMIT_EDITMSG"]);
        assert_eq!(
            n.commit_msg_filename.as_deref(),
            Some(OsStr::new(".git/COMMIT_EDITMSG"))
        );
        let n = ns(HookType::PrepareCommitMsg, &["--", "f", "message", "HEAD"]);
        assert_eq!(
            n.prepare_commit_message_source.as_deref(),
            Some(OsStr::new("message"))
        );
        assert_eq!(n.commit_object_name.as_deref(), Some(OsStr::new("HEAD")));
        let n = ns(HookType::PostMerge, &["--", "1"]);
        assert_eq!(n.is_squash_merge.as_deref(), Some(OsStr::new("1")));
        let n = ns(HookType::PostRewrite, &["--", "amend"]);
        assert_eq!(n.rewrite_command.as_deref(), Some(OsStr::new("amend")));
        let n = ns(HookType::PostCheckout, &["--", "a", "b", "c"]);
        assert_eq!(n.from_ref.as_deref(), Some(OsStr::new("a")));
        assert_eq!(n.to_ref.as_deref(), Some(OsStr::new("b")));
        assert_eq!(n.checkout_type.as_deref(), Some(OsStr::new("c")));
        let n = ns(HookType::PreCommit, &["--"]);
        assert_eq!(n.hook_stage, Stage::Hook(HookType::PreCommit));
        assert_eq!(n.color, ColorSetting::Always);
        assert!(n.files.is_empty() && n.hooks.is_empty() && !n.all_files && !n.verbose);
    }

    #[test]
    fn d_data_07_cdpath_repair() {
        let d = std::env::temp_dir();
        let s = d.to_string_lossy();
        let doubled = OsString::from(format!("{s}\n{s}"));
        assert_eq!(repair_hook_dir(&doubled), d);
        let mixed = OsString::from(format!("{s}\n/nonexistent"));
        assert_eq!(repair_hook_dir(&mixed), PathBuf::from(&mixed));
        assert_eq!(repair_hook_dir(OsStr::new("/x/y")), PathBuf::from("/x/y"));
    }

    #[test]
    fn d_data_07_msys_drive_paths() {
        assert_eq!(msys_drive_path(b"/d/a/x").as_deref(), Some("D:/a/x"));
        assert_eq!(msys_drive_path(b"/c").as_deref(), Some("C:/"));
        assert_eq!(msys_drive_path(b"/c/").as_deref(), Some("C:/"));
        for not_drive in [&b"/usr/bin"[..], b"/1/x", b"d/a", b"/", b""] {
            assert_eq!(msys_drive_path(not_drive), None);
        }
        if cfg!(windows) {
            // the doubled HERE of Git for Windows' shell (the MSYS2 form of the temp directory, twice)
            let d = std::env::temp_dir();
            let s = d.to_string_lossy().replace('\\', "/");
            let (drive, rest) = s.split_once(':').unwrap();
            let msys = format!("/{}{rest}", drive.to_ascii_lowercase());
            let doubled = OsString::from(format!("{msys}\n{msys}"));
            assert_eq!(repair_hook_dir(&doubled), d);
        }
    }

    #[test]
    fn d_sec_12_stdin_cap() {
        let big = vec![b'a'; usize::try_from(STDIN_CAP).unwrap() + 1];
        let e = read_capped(&mut big.as_slice()).unwrap_err();
        match e {
            Failure::Exc { exc, .. } => {
                assert_eq!(exc.ty, "FatalError");
                assert_eq!(
                    String::from_utf8(exc.text).unwrap(),
                    "snp: hook stdin exceeds 64 MiB; refusing to run hooks on a truncated ref list"
                );
            }
            other => panic!("{other:?}"),
        }
        let ok = vec![b'a'; 10];
        assert_eq!(read_capped(&mut ok.as_slice()).unwrap(), ok);
    }
}
