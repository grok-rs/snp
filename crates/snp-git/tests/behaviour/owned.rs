//! Owned repositories: the sanitized environment (D-SEC-05, ADR-018), the prefetch mode and eligibility (ADR-064),
//! local deps clones (ADR-025), Tier B reads, autoupdate facts (03 §11.2) and the try-repo shadow (03 §12).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use snp_core::env::{Env, EnvPatch};
use snp_git::{CloneMode, GitBackend, OwnedRepo, prefetch_eligible};
use snp_proc::spawn::{Captured, Group, PyStatus, SpawnError, Stdin};

use crate::common::Sandbox;

/// One recorded call: argv and environment.
type Call = (Vec<OsString>, BTreeMap<String, String>);

/// Records `(argv, env)` of every call and pretends success (no process is spawned).
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<Call>>,
}

impl Recorder {
    fn record(&self, argv: &[OsString], env: &Env) {
        let vars = env
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        self.calls.lock().unwrap().push((argv.to_vec(), vars));
    }
    fn envs(&self) -> Vec<BTreeMap<String, String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }
}

impl GitBackend for Recorder {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        _: &Path,
        _: Stdin,
        _: Group,
    ) -> Result<Captured, SpawnError> {
        self.record(argv, env);
        Ok(Captured {
            status: PyStatus(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            stopped_by_tty: false,
        })
    }
    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        _: &Path,
        _: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        self.record(argv, env);
        Ok((PyStatus(0), Vec::new()))
    }
    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        _: &Path,
        _: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        self.record(argv, env);
        Ok(PyStatus(0))
    }
}

fn hostile_base(sb: &Sandbox) -> Env {
    sb.env.patched(
        &EnvPatch::default()
            .set("GIT_DIR", "/user/repo/.git")
            .set("GIT_WORK_TREE", "/user/repo")
            .set("GIT_INDEX_FILE", "/user/repo/.git/index")
            .set("GIT_CONFIG_PARAMETERS", "'core.fsmonitor'='true'")
            .set("GIT_CONFIG_COUNT", "1")
            .set("GIT_CONFIG_KEY_0", "http.proxy")
            .set("GIT_CONFIG_VALUE_0", "http://proxy:3128")
            .set("GIT_SSH_COMMAND", "ssh -i key"),
    )
}

#[test]
fn owned_env_is_sanitized_and_carries_the_fsmonitor_injection() {
    let sb = Sandbox::new();
    let rec = Arc::new(Recorder::default());
    let dest = sb.path("dest");
    std::fs::create_dir_all(&dest).unwrap();
    OwnedRepo::clone_rev(
        sb.exe(),
        &hostile_base(&sb),
        &dest,
        "https://h/x",
        "v1",
        CloneMode::Foreground,
        Arc::<Recorder>::clone(&rec),
    )
    .unwrap();
    let envs = rec.envs();
    assert_eq!(
        envs.len(),
        5,
        "init, remote add, fetch, checkout, submodule update"
    );
    for e in &envs {
        for gone in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_CONFIG_PARAMETERS",
        ] {
            assert!(!e.contains_key(gone), "{gone} leaked");
        }
        // a pre-existing count is extended, later entries win
        assert_eq!(e["GIT_CONFIG_COUNT"], "3");
        assert_eq!(e["GIT_CONFIG_KEY_0"], "http.proxy");
        assert_eq!(e["GIT_CONFIG_VALUE_0"], "http://proxy:3128");
        assert_eq!(e["GIT_CONFIG_KEY_1"], "core.fsmonitor");
        assert_eq!(e["GIT_CONFIG_VALUE_1"], "");
        assert_eq!(e["GIT_CONFIG_KEY_2"], "core.useBuiltinFSMonitor");
        assert_eq!(e["GIT_CONFIG_VALUE_2"], "false");
        assert_eq!(e["GIT_SSH_COMMAND"], "ssh -i key");
        assert!(!e.contains_key("GIT_TERMINAL_PROMPT"));
    }
    // argv parity with store.py: -c core.useBuiltinFSMonitor=false only on init and remote add
    let argvs: Vec<String> = rec
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(a, _)| {
            a[1..]
                .iter()
                .map(|x| x.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(
        argvs,
        vec![
            format!(
                "-c core.useBuiltinFSMonitor=false init --template= {}",
                dest.display()
            ),
            "-c core.useBuiltinFSMonitor=false remote add origin https://h/x".to_owned(),
            "-c protocol.version=2 fetch origin v1 --depth=1".to_owned(),
            "checkout FETCH_HEAD".to_owned(),
            "-c protocol.version=2 submodule update --init --recursive --depth=1".to_owned(),
        ]
    );
}

#[test]
fn a_malformed_count_is_left_untouched() {
    let sb = Sandbox::new();
    let rec = Arc::new(Recorder::default());
    let base = sb
        .env
        .patched(&EnvPatch::default().set("GIT_CONFIG_COUNT", "two"));
    OwnedRepo::open(sb.exe(), &base, &sb.root, Arc::<Recorder>::clone(&rec))
        .read_blob("HEAD", "x")
        .unwrap();
    let e = &rec.envs()[0];
    assert_eq!(e["GIT_CONFIG_COUNT"], "two");
    assert!(!e.contains_key("GIT_CONFIG_KEY_0"));
}

#[test]
fn prefetch_adds_exactly_the_non_interactive_variables() {
    let sb = Sandbox::new();
    let run = |repo: &str, mode: CloneMode, base: &Env| {
        let rec = Arc::new(Recorder::default());
        let dest = sb.path(&format!("d{}", rand_suffix(repo, mode)));
        std::fs::create_dir_all(&dest).unwrap();
        OwnedRepo::clone_rev(
            sb.exe(),
            base,
            &dest,
            repo,
            "v1",
            mode,
            Arc::<Recorder>::clone(&rec),
        )
        .unwrap();
        rec.envs()[0].clone()
    };
    let base = sb.env.clone();
    for repo in ["https://github.com/a/b", "git@github.com:a/b.git"] {
        let fg = run(repo, CloneMode::Foreground, &base);
        let pf = run(repo, CloneMode::Prefetch, &base);
        let added: BTreeMap<&String, &String> =
            pf.iter().filter(|(k, v)| fg.get(*k) != Some(v)).collect();
        let mut want: BTreeMap<String, String> = [
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ASKPASS", ""),
            ("GCM_INTERACTIVE", "never"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        if repo.starts_with("git@") {
            want.insert("GIT_SSH_COMMAND".into(), "ssh -o BatchMode=yes".into());
        }
        let added: BTreeMap<String, String> = added
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(added, want, "{repo}");
        assert!(fg.keys().all(|k| pf.contains_key(k)), "nothing is removed");
    }
}

fn rand_suffix(repo: &str, mode: CloneMode) -> String {
    format!("{}-{mode:?}", repo.bytes().map(u32::from).sum::<u32>())
}

#[test]
fn prefetch_eligibility_table() {
    let empty = Env::from_pairs::<&str, &str>([]);
    let ssh_cmd = Env::from_pairs([("GIT_SSH_COMMAND", "ssh -i k")]);
    let ssh = Env::from_pairs([("GIT_SSH", "plink")]);
    for (url, env, core, want) in [
        ("https://github.com/a/b", &empty, false, true),
        ("https://github.com/a/b", &ssh_cmd, true, true),
        ("file:///srv/repos/x", &empty, false, true),
        ("/srv/repos/x", &empty, false, true),
        ("../hooks", &empty, false, true),
        (r"C:\repos\x", &empty, false, true),
        ("git@github.com:a/b.git", &empty, false, true),
        ("ssh://git@github.com/a/b", &empty, false, true),
        ("git@github.com:a/b.git", &ssh_cmd, false, false),
        ("git@github.com:a/b.git", &ssh, false, false),
        ("git@github.com:a/b.git", &empty, true, false),
        ("ssh://h/x", &empty, true, false),
        ("http://example.com/x", &empty, false, false),
        ("git://example.com/x", &empty, false, false),
        ("ext::ssh -i key host %S", &empty, false, false),
    ] {
        assert_eq!(prefetch_eligible(url, env, core), want, "{url} core={core}");
    }
}

/// An upstream with a manifest, two commits, a lightweight `v1.0.0` and an annotated `v2.0.0` (+ `latest`).
fn upstream(sb: &Sandbox) -> (std::path::PathBuf, String, String) {
    let up = sb.init("upstream");
    sb.write(&up, ".pre-commit-hooks.yaml", b"-   id: a\n");
    sb.write(&up, "pkg/mod.py", b"print(1)\n");
    sb.write(&up, "pkg/sub/inner.py", b"x\n");
    sb.git(&up, &["add", "."]);
    sb.commit(&up, "v1");
    sb.git(&up, &["tag", "v1.0.0"]);
    let v1 = sb.head(&up);
    sb.write(&up, ".pre-commit-hooks.yaml", b"-   id: a\n-   id: b\n");
    sb.git(&up, &["add", "."]);
    sb.commit(&up, "v2");
    sb.git(&up, &["tag", "-a", "v2.0.0", "-m", "release"]);
    sb.git(&up, &["tag", "latest"]);
    let v2 = sb.head(&up);
    (up, v1, v2)
}

#[test]
fn rev_info_tags_freeze_and_bleeding_edge() {
    let sb = Sandbox::new();
    let (up, _v1, v2) = upstream(&sb);
    let repo = up.to_str().unwrap();
    let fresh = |name: &str| {
        let d = sb.path(name);
        std::fs::create_dir_all(&d).unwrap();
        OwnedRepo::open(sb.exe(), &sb.env, &d, sb.backend())
    };
    // tags_only: nearest tag, the first `tag --points-at` name containing a dot
    let info = fresh("t1").rev_info(repo, true, false).unwrap();
    assert_eq!(
        (
            info.repo.as_str(),
            info.rev.as_str(),
            info.frozen.as_deref()
        ),
        (repo, "v2.0.0", None)
    );
    assert!(
        info.hook_ids.is_empty(),
        "the caller loads <dir>/.pre-commit-hooks.yaml"
    );
    assert_eq!(
        std::fs::read(sb.path("t1/.pre-commit-hooks.yaml")).unwrap(),
        b"-   id: a\n-   id: b\n"
    );
    // --freeze of an annotated tag gives the tag OBJECT sha (03 §11.2 row 7)
    let info = fresh("t2").rev_info(repo, true, true).unwrap();
    let tag_object = sb.git_text(&up, &["rev-parse", "v2.0.0"]);
    assert_ne!(tag_object, v2);
    assert_eq!(
        (info.rev.as_str(), info.frozen.as_deref()),
        (tag_object.as_str(), Some("v2.0.0"))
    );
    // --bleeding-edge: HEAD is exactly tagged
    let info = fresh("t3").rev_info(repo, false, false).unwrap();
    assert!(info.rev == "v2.0.0" || info.rev == "latest", "{}", info.rev);
    // untagged HEAD: the sha
    sb.commit(&up, "untagged");
    let head = sb.head(&up);
    let info = fresh("t4").rev_info(repo, false, false).unwrap();
    assert_eq!(info.rev, head);
    let info = fresh("t5").rev_info(repo, true, false).unwrap();
    assert_eq!(info.rev, "v2.0.0");
}

#[test]
fn clone_local_makes_a_deps_variant_without_network() {
    let sb = Sandbox::new();
    let (up, v1, _) = upstream(&sb);
    let base_dir = sb.path("base");
    std::fs::create_dir_all(&base_dir).unwrap();
    let base = OwnedRepo::clone_rev(
        sb.exe(),
        &sb.env,
        &base_dir,
        up.to_str().unwrap(),
        "v1.0.0",
        CloneMode::Foreground,
        sb.backend(),
    )
    .unwrap();
    assert_eq!(sb.head(&base_dir), v1);
    let variant_dir = sb.path("variant");
    let variant = OwnedRepo::clone_local(
        sb.exe(),
        &sb.env,
        &base_dir,
        &variant_dir,
        "v1.0.0",
        sb.backend(),
    )
    .unwrap();
    assert_eq!(sb.head(&variant_dir), v1);
    assert_eq!(
        sb.git_text(&variant_dir, &["config", "remote.origin.url"]),
        up.to_str().unwrap()
    );
    assert_eq!(
        std::fs::read(variant_dir.join("pkg/mod.py")).unwrap(),
        b"print(1)\n"
    );
    // Tier B reads work on both
    for repo in [&base, &variant] {
        let blobs = repo.ls_tree_blobs("HEAD", "pkg").unwrap();
        let blob = sb.git_text(&up, &["rev-parse", &format!("{v1}:pkg/mod.py")]);
        assert_eq!(blobs, vec![("pkg/mod.py".to_owned(), blob)]);
        assert_eq!(repo.read_blob("HEAD", "pkg/sub/inner.py").unwrap(), b"x\n");
        assert!(repo.read_blob("HEAD", "missing").is_err());
    }
    let root = base.ls_tree_blobs(&v1, "").unwrap();
    assert_eq!(root.len(), 1, "{root:?}");

    // a broken base falls back to a clone from the base's origin
    let broken = sb.path("broken");
    std::fs::create_dir_all(broken.join(".git")).unwrap();
    std::fs::write(
        broken.join(".git/config"),
        "[remote \"origin\"]\n\turl = nowhere\n",
    )
    .unwrap();
    let fallback_dir = sb.path("fallback");
    std::fs::create_dir_all(&fallback_dir).unwrap();
    let e = OwnedRepo::clone_local(
        sb.exe(),
        &sb.env,
        &broken,
        &fallback_dir,
        "v1.0.0",
        sb.backend(),
    )
    .unwrap_err();
    let (_, text) = crate::common::exc(&e);
    assert!(text.contains("fetch"), "{text}");
}

#[test]
fn shadow_try_repo_commits_uncommitted_changes() {
    let sb = Sandbox::new();
    let user = sb.init("user");
    sb.commit(&user, "user");
    let (git, _) = sb.discover(&user);
    let (up, _, v2) = upstream(&sb);

    // clean: (repo, HEAD) unchanged
    let owned = OwnedRepo::open(sb.exe(), &sb.env, &up, sb.backend());
    let tmp = sb.path("tmp1");
    std::fs::create_dir_all(&tmp).unwrap();
    assert_eq!(
        owned.shadow_try_repo(&git, &tmp).unwrap(),
        (up.clone(), v2.clone())
    );

    // dirty: a shadow commit with the staged and unstaged tracked changes (untracked files are not included)
    sb.write(&up, "pkg/mod.py", b"print(2)\n");
    sb.write(&up, "staged-new.py", b"new\n");
    sb.git(&up, &["add", "staged-new.py"]);
    sb.write(&up, "untracked.py", b"u\n");
    let status_before = sb.git(&up, &["status", "--porcelain=v1"]);
    let tmp = sb.path("tmp2");
    std::fs::create_dir_all(&tmp).unwrap();
    let (shadow, rev) = owned.shadow_try_repo(&git, &tmp).unwrap();
    assert_eq!(shadow, tmp.join("shadow-repo"));
    assert_eq!(sb.head(&shadow), rev);
    assert_ne!(rev, v2);
    let show = |p: &str| {
        sb.git(
            &shadow,
            &[OsStr::new("show"), OsStr::new(&format!("{rev}:{p}"))],
        )
    };
    assert_eq!(show("pkg/mod.py"), b"print(2)\n");
    assert_eq!(show("staged-new.py"), b"new\n");
    assert_ne!(
        sb.git_status(&shadow, &["cat-file", "-e", &format!("{rev}:untracked.py")])
            .0,
        0
    );
    assert_eq!(
        sb.git_text(&shadow, &["log", "-1", "--format=%an <%ae>|%s"]),
        "pre-commit <asottile+pre-commit@umich.edu>|init"
    );
    assert_eq!(
        sb.git(&up, &["status", "--porcelain=v1"]),
        status_before,
        "the tried repo is unchanged"
    );
}
