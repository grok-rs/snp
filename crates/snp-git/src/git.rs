//! The user repository: every git call with pre-commit's exact argv (02 G.1) + only the G.7 rows 1–3 additions.
//!
//! Every call runs with `cwd` = the work-tree root that discovery found and `env` = the run environment (pre-commit
//! exports its `PRE_COMMIT_*` variables into `os.environ`, so its own git calls see them; 02 §6.1). Output is kept as
//! bytes in git's order (`diff.orderFile` parity, D-CRASH-12).
//!
//! Owner: WP-18.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::env::{Env, EnvPatch};
use snp_core::failure::{Failure, pyerr};
use snp_core::path::RepoPath;
use snp_proc::spawn::{Captured, Group, SpawnError, Stdin};
use snp_pycompat::codec::decode_utf8;
use snp_pycompat::precommit::{merge_msg_conflicts, pre_push_lines, pre_push_lines_bytes};
use snp_pycompat::text::{bytes_strip, splitlines, strip};

mod pathspec;

use crate::run::{self, Out, bare_git_argv, concat, os_from_bytes};
use crate::types::{BadRecord, is_null_oid, parse_diff_raw, parse_ls_files_stage};
use crate::zlist::TooLarge;
use crate::{
    DiffFingerprint, DiffIndex, GitBackend, GitExe, IndexEntries, MergeState, ObjectId, PrePushNs,
    RepoInfo, ZList, messages,
};

/// A handle on the user repository; cheap clone (`Arc`s inside).
#[derive(Clone, Debug)]
pub struct Git {
    pub(crate) exe: Arc<GitExe>,
    pub(crate) env: Env,
    pub(crate) root: Arc<Path>,
    pub(crate) admin_dir: Arc<Path>,
    pub(crate) backend: Arc<dyn GitBackend>,
    pub(crate) brand: Brand,
}

fn too_large(_: TooLarge) -> Failure {
    Failure::unexpected("MemoryError", "git output larger than 4 GiB")
}

fn bad_record(what: &str, r: &BadRecord) -> Failure {
    Failure::unexpected(
        "ValueError",
        format!(
            "unexpected `git {what}` output: {}",
            snp_pycompat::repr::repr_fsbytes(&r.0)
        ),
    )
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// The pathspec switches [`Git::checkout_env`] removes.
const PATHSPEC_ENV: [&str; 4] = [
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

/// D-DATA-19: the arguments of the stash's path-limited checkout ([`Git::checkout_paths`]); its stdin is
/// [`literal_pathspecs`] of the paths, its environment [`Git::checkout_env`].
pub const CHECKOUT_PATHS_ARGS: [&str; 5] = [
    "-c",
    "submodule.recurse=0",
    "checkout",
    "--pathspec-from-file=-",
    "--pathspec-file-nul",
];

/// The argument bytes one chunk of [`Git::checkout_path_calls`]'s fallback may carry (well inside Windows' 32 767-character
/// command line and every Unix `ARG_MAX`).
const CHECKOUT_CHUNK_BYTES: usize = 24 * 1024;

/// One git call of the stash's path-limited checkout ([`Git::checkout_path_calls`]): the arguments after `git` and its
/// stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutCall {
    /// The arguments after `git` (the `-c` options included).
    pub args: Vec<OsString>,
    /// The bytes fed on stdin (empty for the argument form).
    pub stdin: Vec<u8>,
}

/// `:(literal)<path>\0` per path: a `--pathspec-file-nul` list in which no name is a glob or carries magic (the
/// verified kernel `pathspec::literal_list`, ADR-095 K7).
pub fn literal_pathspecs<'a>(paths: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let paths: Vec<&[u8]> = paths.into_iter().collect();
    pathspec::literal_list(&paths)
}

/// The paths of `git apply --numstat -z`: records `<added>\t<deleted>\t<path>\0` (`-\t-\t` for a binary file); a
/// rename or copy record (`<added>\t<deleted>\t\0<old>\0<new>\0`, never produced by a stash patch) contributes both
/// names. Unverified shell of the kernel `pathspec::numstat_spans` (ADR-095 K7): every path it returns is non-empty
/// and NUL-free.
fn parse_numstat_z(out: &[u8]) -> Result<ZList, Failure> {
    let spans = pathspec::numstat_spans(out).map_err(|(s, e)| {
        bad_record(
            "apply --numstat -z",
            &BadRecord(out.get(s..e).unwrap_or_default().to_vec()),
        )
    })?;
    let mut list = ZList::default();
    for (s, e) in spans {
        list.push(out.get(s..e).unwrap_or_default())
            .map_err(too_large)?;
    }
    Ok(list)
}

/// The production [`pathspec::SeenSet`]: a hash set over the borrowed records, O(n) for any number of paths. A
/// trusted item of kernel K7 (ADR-095 §2.6, `proofs/registry.toml`), checked against the verified
/// [`pathspec::Scan`] by the differential test `seen_set_hash_matches_scan`.
#[derive(Debug, Clone, Copy)]
struct HashSeen;

impl pathspec::SeenSet for HashSeen {
    fn first_seen(&self, records: &[&[u8]]) -> Vec<bool> {
        let mut seen: std::collections::HashSet<&[u8]> =
            std::collections::HashSet::with_capacity(records.len());
        records.iter().map(|r| seen.insert(*r)).collect()
    }
}

/// Up to this many records the rollback list is deduplicated by the verified [`pathspec::Scan`] (no hashing, no
/// allocation per record); longer lists use [`HashSeen`].
const SCAN_MAX: usize = 16;

/// D-DATA-19 (ADR-093 as amended for ADR-095 K7): the checkout sink's refusal of a path that would select more than
/// itself.
fn refused_checkout_path(path: &[u8]) -> Failure {
    Failure::unexpected(
        "ValueError",
        messages::refused_checkout_path(&snp_pycompat::repr::repr_fsbytes(path)),
    )
}

/// The brand of this process: `basename(argv[0])` (ADR-023, `Brand::detect`), read once.
fn process_brand() -> Brand {
    static BRAND: std::sync::OnceLock<Brand> = std::sync::OnceLock::new();
    *BRAND.get_or_init(|| Brand::detect(std::env::args_os().next().as_deref()))
}

/// `set(a) | set(b)` in first-seen order (pre-commit's `set` order is per-process random): O(n) with a hash set
/// over the borrowed records, whatever the number of conflicted paths.
fn first_seen_union<'a>(records: impl Iterator<Item = &'a [u8]>) -> Result<ZList, TooLarge> {
    let mut seen: std::collections::HashSet<&'a [u8]> = std::collections::HashSet::new();
    let mut out = ZList::default();
    for p in records {
        if seen.insert(p) {
            out.push(p)?;
        }
    }
    Ok(out)
}

/// `bytes.strip()` of a one-line output, as an `OsString`.
fn stripped(b: &[u8]) -> OsString {
    os_from_bytes(bytes_strip(b))
}

/// `stdout` without git's single trailing newline (`rev-parse`, `config --get` print exactly one).
fn chomp(b: &[u8]) -> &[u8] {
    b.trim_suffix(b"\n")
}

/// A sink wrapper that counts bytes, records whether a non-`bytes.isspace()` byte was seen (`stdout.strip()`
/// emptiness) and remembers the sink's own I/O error (so it is not reported as a spawn failure).
struct Counting<'a> {
    inner: &'a mut dyn io::Write,
    written: u64,
    non_ws: bool,
    error: Option<io::ErrorKind>,
}

impl io::Write for Counting<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.inner.write(buf) {
            Ok(n) => {
                let part = buf.get(..n).unwrap_or_default();
                if !self.non_ws {
                    self.non_ws = part
                        .iter()
                        .any(|&b| !snp_pycompat::chars::is_bytes_space(b));
                }
                self.written = self
                    .written
                    .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
                Ok(n)
            }
            Err(e) => {
                self.error = Some(e.kind());
                Err(e)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner
            .flush()
            .inspect_err(|e| self.error = Some(e.kind()))
    }
}

/// The spawn-or-sink failure of a streamed call as Python's `OSError` (exit 3).
fn stream_failure(e: &SpawnError, sink_failed: bool) -> Failure {
    if sink_failed {
        Failure::from_io(&e.io, None, None)
    } else {
        Failure::from_io(&e.io, Some(e.program.as_os_str()), None)
    }
}

/// The pre-push stdin records of `hook_impl._pre_push_ns`, lazily: valid UTF-8 → `str.splitlines()` +
/// `str.rsplit(maxsplit=3)` (PY-WS, ADR-022); otherwise the same on bytes (`bytes.splitlines()`, ASCII whitespace;
/// D-CRASH-11). A malformed line yields its `ValueError` only when reached.
pub(crate) enum PrePushRecords<'a> {
    Text(snp_pycompat::precommit::PrePushLines<'a>),
    Bytes(snp_pycompat::precommit::PrePushLinesBytes<'a>),
}

impl<'a> Iterator for PrePushRecords<'a> {
    type Item = Result<[&'a [u8]; 4], snp_pycompat::text::UnpackError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Text(lines) => Some(lines.next()?.map(|l| {
                [l.local_branch, l.local_sha, l.remote_branch, l.remote_sha].map(str::as_bytes)
            })),
            Self::Bytes(lines) => lines.next(),
        }
    }
}

pub(crate) fn pre_push_records(stdin: &[u8]) -> PrePushRecords<'_> {
    match decode_utf8(stdin) {
        Ok(text) => PrePushRecords::Text(pre_push_lines(text)),
        Err(_) => PrePushRecords::Bytes(pre_push_lines_bytes(stdin)),
    }
}

/// Discovery parsed from `rev-parse` output lines.
struct Discovered {
    cdup: Vec<u8>,
    inside: Vec<u8>,
    git_dir: Vec<u8>,
    common_dir: Vec<u8>,
    index: Vec<u8>,
}

/// The combined call's lines: 5 normally, 4 when `--show-cdup` printed nothing (inside `.git`, bare repository).
/// Anything else (a path containing a newline) means "ask separately".
fn parse_combined(stdout: &[u8]) -> Option<Discovered> {
    let body = stdout.strip_suffix(b"\n")?;
    let lines: Vec<&[u8]> = body.split(|&b| b == b'\n').collect();
    let is_bool = |l: &&[u8]| *l == b"true" || *l == b"false";
    let (cdup, rest): (&[u8], &[&[u8]]) = match lines.as_slice() {
        [cdup, rest @ ..] if rest.len() == 4 && rest.first().is_some_and(is_bool) => (cdup, rest),
        rest if rest.len() == 4 && rest.first().is_some_and(is_bool) => (b"", rest),
        _ => return None,
    };
    let [inside, git_dir, common_dir, index] = rest else {
        return None;
    };
    Some(Discovered {
        cdup: cdup.to_vec(),
        inside: inside.to_vec(),
        git_dir: git_dir.to_vec(),
        common_dir: common_dir.to_vec(),
        index: index.to_vec(),
    })
}

/// `os.path.normpath(os.path.join(base, p))` over raw output bytes.
fn join_norm(base: &Path, p: &[u8]) -> PathBuf {
    let joined = snp_pycompat::ospath::join(base.as_os_str(), &[&os_from_bytes(p)]);
    PathBuf::from(snp_pycompat::ospath::normpath(&joined))
}

impl Git {
    /// One `git rev-parse --show-cdup --is-inside-git-dir --git-dir --git-common-dir --git-path index` (fallback to
    /// separate calls on 2.19–2.30 or on newline-containing paths); `FatalError` texts verbatim (02 §3).
    ///
    /// `git.get_root` (`git.py:51-73`) + `get_git_dir`/`get_git_common_dir` (`git.py:76-93`) + `git_path('index')`
    /// (`git.py:205-207`). On git ≥ 2.31 the combined call adds `--path-format=absolute` before the path options
    /// (02 G.9 item 6); its argv is never shown (every failure is `git failed. Is it installed, …`). A `--git-dir`
    /// echoed back (git < 2.5) is X13-GIT-10's `FatalError`. The returned `Git` renders brand-dependent texts in the
    /// brand of this process (`Brand::detect(argv[0])`, ADR-023), so a caller written against the frozen contract
    /// gets the right `{prog}`; [`Git::with_brand`] overrides it (tests, embedders).
    pub fn discover(
        exe: Arc<GitExe>,
        env: &Env,
        cwd: &Path,
        backend: Arc<dyn GitBackend>,
    ) -> Result<(Self, RepoInfo), Failure> {
        let info = Self::discover_info(&exe, env, cwd, backend.as_ref())?;
        let git = Self {
            exe,
            env: env.clone(),
            root: Arc::from(info.root.as_path()),
            admin_dir: Arc::from(info.git_dir.as_path()),
            backend,
            brand: process_brand(),
        };
        Ok((git, info))
    }

    fn discover_info(
        exe: &GitExe,
        env: &Env,
        cwd: &Path,
        backend: &dyn GitBackend,
    ) -> Result<RepoInfo, Failure> {
        let argv = |args: &[&str]| -> Vec<OsString> {
            let mut v = exe.argv0.clone();
            v.extend(args.iter().map(|a| os(a)));
            v
        };
        let fatal = |msg: &str| Failure::fatal("FatalError", msg.to_owned());
        let combined = if exe.caps.path_format {
            let out = run::capture(
                backend,
                &argv(&[
                    "rev-parse",
                    "--show-cdup",
                    "--is-inside-git-dir",
                    "--path-format=absolute",
                    "--git-dir",
                    "--git-common-dir",
                    "--git-path",
                    "index",
                ]),
                env,
                cwd,
                Group::Inherit,
            );
            if out.status != 0 {
                return Err(fatal(messages::GIT_FAILED));
            }
            parse_combined(&out.stdout)
        } else {
            None
        };
        let d = if let Some(d) = combined {
            d
        } else {
            // get_root(): two calls; any failure is "git failed".
            let cdup = run::capture(
                backend,
                &argv(&["rev-parse", "--show-cdup"]),
                env,
                cwd,
                Group::Inherit,
            );
            if cdup.status != 0 {
                return Err(fatal(messages::GIT_FAILED));
            }
            let inside = run::capture(
                backend,
                &argv(&["rev-parse", "--is-inside-git-dir"]),
                env,
                cwd,
                Group::Inherit,
            );
            if inside.status != 0 {
                return Err(fatal(messages::GIT_FAILED));
            }
            if bytes_strip(&inside.stdout) != b"false" {
                return Err(fatal(messages::TOPLEVEL_EMPTY));
            }
            let root = snp_pycompat::ospath::abspath(&stripped(&cdup.stdout), cwd);
            let at_root = |args: &[&str]| -> Result<Vec<u8>, Failure> {
                let a = argv(args);
                let out = run::checked(backend, &a, env, &root, Group::Inherit)?;
                Ok(bytes_strip(&out.stdout).to_vec())
            };
            Discovered {
                cdup: bytes_strip(&cdup.stdout).to_vec(),
                inside: b"false".to_vec(),
                git_dir: at_root(&["rev-parse", "--git-dir"])?,
                common_dir: at_root(&["rev-parse", "--git-common-dir"])?,
                index: at_root(&["rev-parse", "--git-path", "index"])?,
            }
        };
        if d.inside != b"false" {
            return Err(fatal(messages::TOPLEVEL_EMPTY));
        }
        let root = snp_pycompat::ospath::abspath(&os_from_bytes(&d.cdup), cwd);
        if d.git_dir == b"--git-dir" {
            // git < 2.5: `AssertionError: unreachable: no git dir` in pre-commit (X13-GIT-10).
            let exe_name = exe.argv0.first().map_or_else(|| os("git"), Clone::clone);
            return Err(Failure::fatal(
                "FatalError",
                messages::git_dir_unsupported(&exe.version_text(), &exe_name),
            ));
        }
        let git_dir = join_norm(&root, &d.git_dir);
        let common_dir = if d.common_dir == b"--git-common-dir" {
            git_dir.clone() // `git.py:92`: git < 2.5
        } else {
            join_norm(&root, &d.common_dir)
        };
        let index_file = if d.index.starts_with(b"--git-path") {
            env.get_nonempty("GIT_INDEX_FILE").map_or_else(
                || git_dir.join("index"),
                |p| join_norm(&root, p.as_encoded_bytes()),
            )
        } else {
            join_norm(&root, &d.index)
        };
        Ok(RepoInfo {
            is_worktree: git_dir != common_dir,
            root,
            git_dir,
            common_dir,
            index_file,
        })
    }

    /// Run vars exported (pre-commit sets `os.environ`, `run.py:375-417`): every later call sees `env`.
    #[must_use]
    pub fn with_env(&self, env: Env) -> Self {
        Self {
            env,
            ..self.clone()
        }
    }

    /// The stash's Critical backend (ADR-065).
    #[must_use]
    pub fn with_backend(&self, b: Arc<dyn GitBackend>) -> Self {
        Self {
            backend: b,
            ..self.clone()
        }
    }

    /// The brand of the texts this handle renders (`{prog} failed to diff …`, COMPATIBILITY §3.3); `discover`
    /// starts with the compat brand. Derived handles (`with_env`, `with_backend`) keep it.
    #[must_use]
    pub fn with_brand(&self, brand: Brand) -> Self {
        Self {
            brand,
            ..self.clone()
        }
    }

    /// The environment of every call.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The work-tree root (cwd of every call).
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn argv<S: AsRef<OsStr>>(&self, args: &[S]) -> Vec<OsString> {
        let mut v = self.exe.argv0.clone();
        v.extend(args.iter().map(|a| a.as_ref().to_os_string()));
        v
    }

    fn capture<S: AsRef<OsStr>>(&self, args: &[S], env: &Env, group: Group) -> Out {
        run::capture(
            self.backend.as_ref(),
            &self.argv(args),
            env,
            &self.root,
            group,
        )
    }

    fn checked<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Out, Failure> {
        run::checked(
            self.backend.as_ref(),
            &self.argv(args),
            &self.env,
            &self.root,
            Group::Inherit,
        )
    }

    fn checked_zlist<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<ZList, Failure> {
        ZList::from_z(self.checked(args)?.stdout).map_err(too_large)
    }

    /// `_has_unmerged_paths` (`run.py:325-327`): `git ls-files --unmerged` (checked), stdout non-blank.
    pub fn has_unmerged(&self) -> Result<bool, Failure> {
        Ok(!bytes_strip(&self.checked(&["ls-files", "--unmerged"])?.stdout).is_empty())
    }

    /// `_has_unstaged_config` (`run.py:330-334`): `git diff --quiet --no-ext-diff <cfg>` (no `--`), rc == 1 exactly;
    /// any other returncode (128 for a missing or outside path) is "staged".
    pub fn config_is_unstaged(&self, cfg: &OsStr) -> Result<bool, Failure> {
        let args = [
            os("diff"),
            os("--quiet"),
            os("--no-ext-diff"),
            cfg.to_os_string(),
        ];
        Ok(self.capture(&args, &self.env, Group::Inherit).status == 1)
    }

    /// `git.intent_to_add_files` (`git.py:146-151`) + `--no-renames` (hazard b, D-DATA-12):
    /// `git diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z` (checked).
    pub fn intent_to_add(&self) -> Result<ZList, Failure> {
        self.checked_zlist(&[
            "diff",
            "--no-renames",
            "--no-ext-diff",
            "--ignore-submodules",
            "--diff-filter=A",
            "--name-only",
            "-z",
        ])
    }

    fn zlist_os(paths: &ZList) -> Vec<OsString> {
        paths.iter().map(os_from_bytes).collect()
    }

    /// `xargs(('git', 'rm', '--cached', '--'), files)` (`staged_files_only.py:41`): partitioned by command length,
    /// serial, returncode ignored.
    pub fn rm_cached(&self, paths: &ZList, group: Group) -> Result<(), Failure> {
        let files = Self::zlist_os(paths);
        let refs: Vec<&OsStr> = files.iter().map(OsString::as_os_str).collect();
        run::xargs_serial(
            self.backend.as_ref(),
            &self.argv(&["rm", "--cached", "--"]),
            &refs,
            &self.env,
            &self.root,
            group,
        )
        .map(drop)
    }

    /// `xargs(('git', 'add', '--intent-to-add', '--'), files)` (`staged_files_only.py:45`): returncode ignored.
    pub fn add_intent_to_add(&self, paths: &ZList, group: Group) -> Result<(), Failure> {
        let files = Self::zlist_os(paths);
        let refs: Vec<&OsStr> = files.iter().map(OsString::as_os_str).collect();
        run::xargs_serial(
            self.backend.as_ref(),
            &self.argv(&["add", "--intent-to-add", "--"]),
            &refs,
            &self.env,
            &self.root,
            group,
        )
        .map(drop)
    }

    /// `git write-tree` (checked, `staged_files_only.py:52`), stripped.
    pub fn write_tree(&self) -> Result<ObjectId, Failure> {
        let out = self.checked(&["write-tree"])?;
        let text = String::from_utf8_lossy(&out.stdout);
        ObjectId::parse(strip(&text)).ok_or_else(|| {
            Failure::unexpected(
                "ValueError",
                format!(
                    "`git write-tree` printed {}",
                    snp_pycompat::repr::repr_str(&text)
                ),
            )
        })
    }

    /// `GIT_DIFF_OPTS` removed (hazard a, D-DATA-11); stdout streamed into `sink`; `FatalError` text for other rcs
    /// (02 §7).
    ///
    /// `git diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff <tree> --`
    /// (`staged_files_only.py:53-104`): rc 0 → [`DiffIndex::Clean`]; rc 1 with `stdout.strip()` empty →
    /// [`DiffIndex::CrlfQuirk`]; rc 1 → [`DiffIndex::Patch`]; any other rc →
    /// `FatalError: {prog} failed to diff -- perhaps due to permissions?` + the format-A `CalledProcessError` of the
    /// bare-`git` argv with empty stdout. A failing `sink` is its own `OSError` (exit 3).
    pub fn diff_index_patch(
        &self,
        tree: &ObjectId,
        sink: &mut dyn io::Write,
    ) -> Result<DiffIndex, Failure> {
        let args: Vec<OsString> = [
            "diff-index",
            "--ignore-submodules",
            "--binary",
            "--exit-code",
            "--no-color",
            "--no-ext-diff",
            tree.as_str(),
            "--",
        ]
        .iter()
        .map(|a| os(a))
        .collect();
        let env = self.env.without(&["GIT_DIFF_OPTS"]);
        let mut counting = Counting {
            inner: sink,
            written: 0,
            non_ws: false,
            error: None,
        };
        let (status, stderr) = self
            .backend
            .stream(&self.argv(&args), &env, &self.root, &mut counting)
            .map_err(|e| stream_failure(&e, counting.error.is_some()))?;
        match status.0 {
            0 => Ok(DiffIndex::Clean),
            1 if !counting.non_ws => Ok(DiffIndex::CrlfQuirk),
            1 => Ok(DiffIndex::Patch {
                bytes_written: counting.written,
            }),
            rc => Err(Failure::fatal(
                "FatalError",
                messages::diff_failed(
                    self.brand,
                    &pyerr::called_process_a(&bare_git_argv(&args), rc, Some(b""), Some(&stderr)),
                ),
            )),
        }
    }

    /// The environment of [`Git::checkout_paths`]: the pathspec switches removed (`GIT_LITERAL_PATHSPECS=1` would make
    /// the `:(literal)` prefix part of every name, `GIT_ICASE_PATHSPECS` / `GIT_GLOB_PATHSPECS` /
    /// `GIT_NOGLOB_PATHSPECS` would change or reject it), plus `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` when
    /// `skip_post_checkout` (#1418).
    pub fn checkout_env(&self, skip_post_checkout: bool) -> Env {
        let env = self.env.without(&PATHSPEC_ENV);
        if skip_post_checkout {
            env.patched(&EnvPatch::default().set("_PRE_COMMIT_SKIP_POST_CHECKOUT", "1"))
        } else {
            env
        }
    }

    /// D-DATA-19: `git -c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul` fed the
    /// [`literal_pathspecs`] of `paths` (checked), in [`Git::checkout_env`]. Replaces pre-commit's whole-tree `git -c
    /// submodule.recurse=0 checkout -- .` (`staged_files_only.py:23,81,95`): only `paths` are rewritten from the index,
    /// so a working-tree change that `git diff` cannot see (hidden by a clean filter, or in an `--assume-unchanged`
    /// file) is never overwritten. No argv limit (the list is on stdin), every name literal. An empty list runs
    /// nothing.
    pub fn checkout_paths(
        &self,
        paths: &ZList,
        skip_post_checkout: bool,
        group: Group,
    ) -> Result<(), Failure> {
        let env = self.checkout_env(skip_post_checkout);
        for call in self.checkout_path_calls(paths)? {
            let args: Vec<&OsStr> = call.args.iter().map(OsString::as_os_str).collect();
            let argv = self.argv(&args);
            let stdin = Stdin::Bytes(Arc::from(call.stdin));
            let out = match self.backend.capture(&argv, &env, &self.root, stdin, group) {
                Ok(c) => Out {
                    status: c.status.0,
                    stdout: c.stdout,
                    stderr: Some(c.stderr),
                },
                Err(e) => Out::from_spawn_error(&e),
            };
            if out.status != 0 {
                return Err(out.called_process_error(&argv));
            }
        }
        Ok(())
    }

    /// D-DATA-19: the calls that check out exactly `paths`, every name a `:(literal)` pathspec: one
    /// [`CHECKOUT_PATHS_ARGS`] call fed the [`literal_pathspecs`] on stdin (git ≥ 2.25), else — git before 2.25 has no
    /// `--pathspec-from-file` — `git -c submodule.recurse=0 checkout -- :(literal)<path>…` in chunks of at most
    /// 24 KiB of arguments (a name longer than that gets a call of its own). Run them in the
    /// environment of [`Git::checkout_env`]. No call for an empty list.
    ///
    /// A list holding an empty path (an empty `:(literal)` pathspec selects the whole tree, the checkout D-DATA-19
    /// removed) or a path with a NUL byte (it would split into two items) is refused (`ValueError`) before any call:
    /// git-made lists never hold one. Unverified shell of the kernels `pathspec::checkout_refusal`, `literal_list`,
    /// `literal_chunks` and `literal_spec` (ADR-095 K7).
    pub fn checkout_path_calls(&self, paths: &ZList) -> Result<Vec<CheckoutCall>, Failure> {
        let list: Vec<&[u8]> = paths.iter().collect();
        if let Some(k) = pathspec::checkout_refusal(&list) {
            return Err(refused_checkout_path(
                list.get(k).copied().unwrap_or_default(),
            ));
        }
        if list.is_empty() {
            return Ok(Vec::new());
        }
        if self.exe.caps.pathspec_from_file {
            return Ok(vec![CheckoutCall {
                args: CHECKOUT_PATHS_ARGS.iter().map(|a| os(a)).collect(),
                stdin: pathspec::literal_list(&list),
            }]);
        }
        Ok(pathspec::literal_chunks(&list, CHECKOUT_CHUNK_BYTES)
            .into_iter()
            .map(|(start, end)| {
                let mut args: Vec<OsString> = ["-c", "submodule.recurse=0", "checkout", "--"]
                    .iter()
                    .map(|a| os(a))
                    .collect();
                args.extend(
                    list.get(start..end)
                        .unwrap_or_default()
                        .iter()
                        .map(|p| os_from_bytes(&pathspec::literal_spec(p))),
                );
                CheckoutCall {
                    args,
                    stdin: Vec::new(),
                }
            })
            .collect())
    }

    /// D-DATA-19: the paths a stash patch touches, from git's own patch parser: `git apply --numstat -z <patch>`
    /// (checked; it reads the patch and changes nothing). Exactly the files whose content `git diff-index` found
    /// different — never a file that is only stat-dirty, which `diff-index --name-only` would list.
    pub fn patch_paths(&self, patch: &Path) -> Result<ZList, Failure> {
        // `--whitespace=nowarn`, as on every apply of the stash (`staged_files_only.py:26-33`): with
        // `apply.whitespace=error` a patch adding trailing whitespace would otherwise fail the listing (exit 128).
        let args = [
            os("apply"),
            os("--numstat"),
            os("-z"),
            os("--whitespace=nowarn"),
            patch.as_os_str().to_os_string(),
        ];
        parse_numstat_z(&self.checked(&args)?.stdout)
    }

    /// D-DATA-19: the files whose working-tree content differs from the index as `git diff` sees it after refreshing
    /// the stat data — what a whole-tree checkout visibly changes, without the files whose differences git cannot see:
    /// `git diff --name-only -z --no-renames --no-ext-diff --no-color --ignore-submodules --no-relative` (checked).
    pub fn changed_paths(&self) -> Result<ZList, Failure> {
        let mut args = vec![
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-color",
            "--ignore-submodules",
        ];
        // `diff.relative` (git ≥ 2.28) would list only the files under the cwd, relative to it; older git has no such
        // setting and no `--no-relative`.
        if self.exe.caps.diff_no_relative {
            args.push("--no-relative");
        }
        self.checked_zlist(&args)
    }

    /// D-DATA-19: what the stash rollback checks out — the patch's paths (so it applies again) followed by every other
    /// path [`Git::changed_paths`] lists (so every hook fix git can see is discarded, as pre-commit's whole-tree
    /// checkout discards it), each once; an empty record of the listing (git never prints one) is dropped. Unverified
    /// shell of the kernel `pathspec::rollback_list` (ADR-095 K7).
    pub fn rollback_paths(&self, patch: &Path) -> Result<ZList, Failure> {
        let touched = self.patch_paths(patch)?;
        let changed = self.changed_paths()?;
        let touched: Vec<&[u8]> = touched.iter().collect();
        let changed: Vec<&[u8]> = changed.iter().collect();
        let list = if touched.len().saturating_add(changed.len()) <= SCAN_MAX {
            pathspec::rollback_list(&pathspec::Scan, &touched, &changed)
        } else {
            pathspec::rollback_list(&HashSeen, &touched, &changed)
        };
        let mut out = ZList::default();
        for p in list {
            out.push(p).map_err(too_large)?;
        }
        Ok(out)
    }

    /// One `_git_apply` attempt (`staged_files_only.py:26-33`): `git apply --whitespace=nowarn <patch>` or, with
    /// `autocrlf_false`, `git -c core.autocrlf=false apply --whitespace=nowarn <patch>`.
    ///
    /// `Ok(true)` = applied. A failing plain attempt is `Ok(false)` (pre-commit catches it and retries); a failing
    /// `autocrlf_false` attempt is the checked call's `CalledProcessError` (exit 3 text of X13-STASH-02), exactly as
    /// `_git_apply` raises it — the caller decides whether to roll back or to report it.
    pub fn apply(&self, patch: &Path, autocrlf_false: bool, group: Group) -> Result<bool, Failure> {
        let mut args: Vec<OsString> = Vec::new();
        if autocrlf_false {
            args.extend([os("-c"), os("core.autocrlf=false")]);
        }
        args.extend([
            os("apply"),
            os("--whitespace=nowarn"),
            patch.as_os_str().to_os_string(),
        ]);
        let argv = self.argv(&args);
        let out = run::capture(self.backend.as_ref(), &argv, &self.env, &self.root, group);
        match (out.status, autocrlf_false) {
            (0, _) => Ok(true),
            (_, false) => Ok(false),
            (_, true) => Err(out.called_process_error(&argv)),
        }
    }

    /// `git apply --check --whitespace=nowarn <patch>` (`stage_fixed`, snp-only; the same whitespace mode as the
    /// restore's apply): would the user's patch still apply on top of the hook fixes.
    pub fn apply_check(&self, patch: &Path) -> Result<bool, Failure> {
        let args = [
            os("apply"),
            os("--check"),
            os("--whitespace=nowarn"),
            patch.as_os_str().to_os_string(),
        ];
        Ok(self.capture(&args, &self.env, Group::Inherit).status == 0)
    }

    /// `git --literal-pathspecs add -- <paths>` (`stage_fixed`, snp-only, ARCHITECTURE §4.5): partitioned like
    /// `xargs`, each partition checked.
    pub fn add_literal(&self, paths: &[RepoPath], group: Group) -> Result<(), Failure> {
        let files: Vec<OsString> = paths.iter().map(|p| p.to_os().into_owned()).collect();
        let refs: Vec<&OsStr> = files.iter().map(OsString::as_os_str).collect();
        run::xargs_checked(
            self.backend.as_ref(),
            &self.argv(&["--literal-pathspecs", "add", "--"]),
            &refs,
            &self.env,
            &self.root,
            group,
        )
    }

    /// `git.get_staged_files` (`git.py:135-143`) + `--no-renames` (output-neutral, 02 G.7 row 3):
    /// `git diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB --no-renames` (checked).
    pub fn staged_files(&self) -> Result<ZList, Failure> {
        self.checked_zlist(&[
            "diff",
            "--staged",
            "--name-only",
            "--no-ext-diff",
            "-z",
            "--diff-filter=ACMRTUXB",
            "--no-renames",
        ])
    }

    /// `git.get_all_files` (`git.py:154-155`): `git ls-files -z --deduplicate`, or on git < 2.31 `git ls-files -z` +
    /// adjacent dedup (byte-identical list; D-CRASH-26).
    pub fn all_files(&self) -> Result<ZList, Failure> {
        if self.exe.caps.ls_files_dedup {
            self.checked_zlist(&["ls-files", "-z", "--deduplicate"])
        } else {
            ZList::from_z_dedup(self.checked(&["ls-files", "-z"])?.stdout).map_err(too_large)
        }
    }

    /// `git ls-files -s -z` (checked): every index entry with its mode, unmerged stages included (ADR-079).
    pub fn tracked_with_modes(&self) -> Result<IndexEntries, Failure> {
        let out = self.checked(&["ls-files", "-s", "-z"])?;
        parse_ls_files_stage(out.stdout).map_err(|r| bad_record("ls-files -s -z", &r))
    }

    /// `git diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB` (checked): the staged paths with their index
    /// modes (ADR-079).
    pub fn staged_with_modes(&self) -> Result<IndexEntries, Failure> {
        let out = self.checked(&[
            "diff",
            "--staged",
            "--raw",
            "-z",
            "--no-renames",
            "--diff-filter=ACMRTUXB",
        ])?;
        parse_diff_raw(out.stdout).map_err(|r| bad_record("diff --staged --raw -z", &r))
    }

    /// Entry count from the index header (`DIRC`, version, count; 12 bytes read, no spawn) — Stat-mode gate.
    ///
    /// `None` when the index is missing, unreadable or not a version 2–4 index. A split index (`core.splitIndex`)
    /// counts only the entries of the main file: an underestimate the caller's own `ls-files -s -z` corrects.
    pub fn index_entry_count(&self, info: &RepoInfo) -> Option<u32> {
        let mut header = [0_u8; 12];
        std::fs::File::open(&info.index_file)
            .ok()?
            .read_exact(&mut header)
            .ok()?;
        let (magic, rest) = header.split_first_chunk::<4>()?;
        let (version, rest) = rest.split_first_chunk::<4>()?;
        let (count, _) = rest.split_first_chunk::<4>()?;
        let version = u32::from_be_bytes(*version);
        (magic == b"DIRC" && (2..=4).contains(&version)).then(|| u32::from_be_bytes(*count))
    }

    /// `git.get_changed_files` (`git.py:158-167`): `git diff --name-only --no-ext-diff -z <from>...<to>`; on a
    /// `CalledProcessError` (no merge base) the two-dot form, whose error is the one reported (X13-GIT-04). No
    /// `--no-renames`: it would change the list (02 G.7).
    pub fn changed_files(&self, from: &OsStr, to: &OsStr) -> Result<ZList, Failure> {
        let diff = |sep: &str| -> Result<ZList, Failure> {
            let range = concat(&[from, OsStr::new(sep), to]);
            let args = [
                os("diff"),
                os("--name-only"),
                os("--no-ext-diff"),
                os("-z"),
                range,
            ];
            self.checked_zlist(&args)
        };
        diff("...").or_else(|_| diff(".."))
    }

    /// `git.is_in_merge_conflict` (`git.py:96-101`): `MERGE_MSG` and `MERGE_HEAD` both exist in the git dir (a
    /// cherry-pick conflict has no `MERGE_MSG`+`MERGE_HEAD` pair).
    pub fn merge_state(&self, info: &RepoInfo) -> MergeState {
        if info.git_dir.join("MERGE_MSG").exists() && info.git_dir.join("MERGE_HEAD").exists() {
            MergeState::Merging
        } else {
            MergeState::None
        }
    }

    /// `git.get_conflicted_files` (`git.py:114-132`): the tab-indented `MERGE_MSG` lines (parsed on bytes, never
    /// decoded: D-CRASH-12, X13-GIT-07) ∪ `git diff --name-only --no-ext-diff -z -m <write-tree> HEAD MERGE_HEAD --`,
    /// in first-seen order (pre-commit's `set` order is per-process random: COMPATIBILITY N-1, a determinism note the
    /// difftest compares with `lines-unordered`). The `[INFO] Checking merge-conflict files only.` line is the
    /// caller's.
    pub fn conflicted_files(&self, info: &RepoInfo) -> Result<ZList, Failure> {
        let msg_path = info.git_dir.join("MERGE_MSG");
        let merge_msg = std::fs::read(&msg_path).map_err(|e| {
            // os.path.join(get_git_dir('.'), 'MERGE_MSG'): relative to the root when git printed a relative dir.
            let shown = info
                .git_dir
                .strip_prefix(&info.root)
                .map_or_else(|_| msg_path.clone(), |rel| rel.join("MERGE_MSG"));
            Failure::from_io(&e, Some(shown.as_os_str()), None)
        })?;
        let tree = self.write_tree()?;
        let diff = self.checked_zlist(&[
            "diff",
            "--name-only",
            "--no-ext-diff",
            "-z",
            "-m",
            tree.as_str(),
            "HEAD",
            "MERGE_HEAD",
            "--",
        ])?;
        first_seen_union(
            merge_msg_conflicts(&merge_msg)
                .into_iter()
                .chain(diff.iter()),
        )
        .map_err(too_large)
    }

    /// `run._get_diff` (`run.py:274-279`): `git diff --no-ext-diff --no-textconv --ignore-submodules` (unchecked,
    /// stderr discarded), hashed with blake3 while streaming (O(1) memory). A spawn failure hashes the text pre-commit
    /// would compare (`_oserror_to_output`).
    pub fn diff_fingerprint(&self) -> Result<DiffFingerprint, Failure> {
        let mut hasher = blake3::Hasher::new();
        let mut counting = Counting {
            inner: &mut hasher,
            written: 0,
            non_ws: false,
            error: None,
        };
        let argv = self.argv(&[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules",
        ]);
        match self
            .backend
            .stream(&argv, &self.env, &self.root, &mut counting)
        {
            Ok(_) => {}
            Err(e) if counting.error.is_some() => return Err(stream_failure(&e, true)),
            Err(e) => {
                let text = Out::from_spawn_error(&e).stdout;
                io::Write::write_all(&mut counting, &text)
                    .map_err(|e| Failure::from_io(&e, None, None))?;
            }
        }
        let non_empty = counting.written > 0;
        Ok(DiffFingerprint {
            digest: *hasher.finalize().as_bytes(),
            non_empty,
        })
    }

    /// `--show-diff-on-failure` (`run.py:317-320`): `subprocess.call(('git', '--no-pager', 'diff', '--no-ext-diff',
    /// '--color=always'|'--color=never'))` with inherited stdio; the returncode is ignored. The caller flushes the
    /// Printer first.
    pub fn show_diff(&self, color: bool) -> Result<(), Failure> {
        let color_arg = if color {
            "--color=always"
        } else {
            "--color=never"
        };
        let argv = self.argv(&["--no-pager", "diff", "--no-ext-diff", color_arg]);
        self.backend
            .inherit(&argv, &self.env, &self.root, Stdin::Inherit)
            .map(drop)
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))
    }

    /// `git.has_core_hookpaths_set` (`git.py:179-182`): `git config core.hooksPath` (unchecked); the stripped value
    /// when non-blank.
    pub fn hooks_path(&self) -> Result<Option<OsString>, Failure> {
        let out = self.capture(&["config", "core.hooksPath"], &self.env, Group::Inherit);
        let v = bytes_strip(&out.stdout);
        Ok((!v.is_empty()).then(|| os_from_bytes(v)))
    }

    /// `git config --list --show-origin -z` (checked; snp-only, `DiffOracle` inputs): the distinct origin files in
    /// first-seen order, relative ones joined to the root. Non-file origins (`command line:`, `blob:`, …) are skipped.
    pub fn config_origins(&self) -> Result<Vec<PathBuf>, Failure> {
        let records = self.checked_zlist(&["config", "--list", "--show-origin", "-z"])?;
        let mut out: Vec<PathBuf> = Vec::new();
        let mut it = records.iter();
        while let Some(origin) = it.next() {
            let _key_value: Option<&[u8]> = it.next();
            if let Some(file) = origin.strip_prefix(b"file:") {
                let p = join_norm(&self.root, file);
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        Ok(out)
    }

    /// `git config --get <key>`: `Some(value)` (rc 0), `None` (rc 1: unset); other returncodes are the checked
    /// call's `CalledProcessError`.
    pub fn config_get(&self, key: &str) -> Result<Option<OsString>, Failure> {
        let argv = self.argv(&["config", "--get", key]);
        let out = run::capture(
            self.backend.as_ref(),
            &argv,
            &self.env,
            &self.root,
            Group::Inherit,
        );
        match out.status {
            0 => Ok(Some(os_from_bytes(chomp(&out.stdout)))),
            1 => Ok(None),
            _ => Err(out.called_process_error(&argv)),
        }
    }

    /// `git config -z --get-regexp <pattern>` (02 G.5 double-run detection): `(key, value)` pairs in config order;
    /// a valueless key has an empty value; rc 1 (no match) is empty.
    pub fn config_get_regexp(&self, pattern: &str) -> Result<Vec<(OsString, OsString)>, Failure> {
        let argv = self.argv(&["config", "-z", "--get-regexp", pattern]);
        let out = run::capture(
            self.backend.as_ref(),
            &argv,
            &self.env,
            &self.root,
            Group::Inherit,
        );
        match out.status {
            0 => {}
            1 => return Ok(Vec::new()),
            _ => return Err(out.called_process_error(&argv)),
        }
        let records = ZList::from_z(out.stdout).map_err(too_large)?;
        Ok(records
            .iter()
            .map(|r| match memchr::memchr(b'\n', r) {
                Some(i) => (
                    os_from_bytes(r.get(..i).unwrap_or_default()),
                    os_from_bytes(r.get(i.saturating_add(1)..).unwrap_or_default()),
                ),
                None => (os_from_bytes(r), OsString::new()),
            })
            .collect())
    }

    /// `hook_impl._pre_push_ns` (`hook_impl.py:120-174`, 02 §16.3, ADR-022) + X13-HI-04.
    ///
    /// Valid UTF-8: Python `str.splitlines()` + `line.rsplit(maxsplit=3)`; a malformed line is
    /// `ValueError: not enough values to unpack (expected 4, got N)` (exit 3, X13-HI-03), raised only when reached.
    /// Non-UTF-8 stdin runs the same algorithm on bytes (D-CRASH-11). A deletion (all-zero local OID, 40 **or** 64
    /// digits: D-CRASH-10) is skipped; a remote OID present locally (`git cat-file -e <sha>^{commit}`, stdio
    /// inherited: its `fatal: Not a valid object name …` reaches stderr, X13-SM-05) gives `from_ref`/`to_ref`;
    /// otherwise `git rev-list <local> --topo-order --reverse --not --remotes=<remote>` decides between "nothing new"
    /// (next line), a whole new history (`all_files`) and `from_ref = <first new>^`. The first line that yields a
    /// namespace wins. The `rev-list`/`rev-parse` failures are the stdlib `CalledProcessError` (format B, bare `git`);
    /// git's stderr goes to the process stderr, raw and before the exception is reported, as Python leaves it
    /// inherited.
    pub fn pre_push(
        &self,
        stdin: &[u8],
        remote_name: &OsStr,
    ) -> Result<Option<PrePushNs>, Failure> {
        for line in pre_push_records(stdin) {
            let fields = line.map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?;
            if let Some(ns) = self.pre_push_line(fields, remote_name)? {
                return Ok(Some(ns));
            }
        }
        Ok(None)
    }

    fn pre_push_line(
        &self,
        [local_branch, local_sha, remote_branch, remote_sha]: [&[u8]; 4],
        remote_name: &OsStr,
    ) -> Result<Option<PrePushNs>, Failure> {
        if is_null_oid(local_sha) {
            return Ok(None);
        }
        let ns = |all_files: bool, from: Option<OsString>, to: Option<OsString>| PrePushNs {
            all_files,
            from_ref: from,
            to_ref: to,
            local_branch: os_from_bytes(local_branch),
            remote_branch: os_from_bytes(remote_branch),
        };
        let local = os_from_bytes(local_sha);
        if !is_null_oid(remote_sha) && self.rev_exists(remote_sha)? {
            return Ok(Some(ns(
                false,
                Some(os_from_bytes(remote_sha)),
                Some(local),
            )));
        }
        let ancestors = self.check_output(&[
            os("rev-list"),
            local.clone(),
            os("--topo-order"),
            os("--reverse"),
            os("--not"),
            concat(&[OsStr::new("--remotes="), remote_name]),
        ])?;
        let ancestors = String::from_utf8_lossy(&ancestors);
        let ancestors = strip(&ancestors);
        let Some(first) = splitlines(ancestors).next() else {
            return Ok(None); // nothing new on this ref
        };
        let roots_out =
            self.check_output(&[os("rev-list"), os("--max-parents=0"), local.clone()])?;
        let roots_text = String::from_utf8_lossy(&roots_out);
        if splitlines(&roots_text).any(|r| r == first) {
            return Ok(Some(ns(true, None, None)));
        }
        let source = self.check_output(&[os("rev-parse"), os(&format!("{first}^"))])?;
        let source = String::from_utf8_lossy(&source);
        Ok(Some(ns(false, Some(os(strip(&source))), Some(local))))
    }

    /// `_rev_exists` (`hook_impl.py:116-117`): `subprocess.call(('git', 'cat-file', '-e', f'{rev}^{{commit}}'))`.
    fn rev_exists(&self, rev: &[u8]) -> Result<bool, Failure> {
        let mut spec = os_from_bytes(rev);
        spec.push("^{commit}");
        let argv = self.argv(&[os("cat-file"), os("-e"), spec]);
        self.backend
            .inherit(&argv, &self.env, &self.root, Stdin::Inherit)
            .map(|st| st.0 == 0)
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))
    }

    /// `subprocess.check_output(('git', *args))`: stdout; a non-zero returncode is the stdlib
    /// `CalledProcessError` (format B, bare `git`; 13 X.2.2).
    ///
    /// Python leaves git's stderr inherited, so its raw bytes reach the terminal before pre-commit prints the
    /// exception: the captured stderr is written, unmodified, to the process stderr as soon as the call returns
    /// (success or failure), never into the exception.
    fn check_output(&self, args: &[OsString]) -> Result<Vec<u8>, Failure> {
        self.check_output_to(args, &mut io::stderr().lock())
    }

    /// [`Git::check_output`] with git's stderr forwarded to `stderr`.
    fn check_output_to(
        &self,
        args: &[OsString],
        stderr: &mut dyn io::Write,
    ) -> Result<Vec<u8>, Failure> {
        let argv = self.argv(args);
        let c = self
            .backend
            .capture(&argv, &self.env, &self.root, Stdin::Null, Group::Inherit)
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))?;
        if !c.stderr.is_empty() {
            // best effort, like an inherited descriptor: a closed stderr loses the text, never the result
            let _ignored: io::Result<()> =
                stderr.write_all(&c.stderr).and_then(|()| stderr.flush());
        }
        if c.status.0 == 0 {
            return Ok(c.stdout);
        }
        Err(Failure::unexpected(
            "CalledProcessError",
            pyerr::called_process_b(&bare_git_argv(args), c.status.0),
        ))
    }

    /// Raw call with the hook env for pre-commit-hooks emulation (exact upstream argv, 09 §3, §9.6): `git <args>` in
    /// the root, stdin `/dev/null`, unchecked. A spawn failure is Python's `OSError` for `'git'`.
    pub fn run_raw(&self, args: &[&OsStr]) -> Result<Captured, Failure> {
        self.backend
            .capture(
                &self.argv(args),
                &self.env,
                &self.root,
                Stdin::Null,
                Group::Inherit,
            )
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))
    }

    /// [`Git::run_raw`] with `input` on stdin (the stash rollback's [`CHECKOUT_PATHS_ARGS`] call, D-DATA-19).
    pub fn run_raw_input(&self, args: &[&OsStr], input: Vec<u8>) -> Result<Captured, Failure> {
        self.backend
            .capture(
                &self.argv(args),
                &self.env,
                &self.root,
                Stdin::Bytes(Arc::from(input)),
                Group::Inherit,
            )
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        let nib = |c: u8| u8::try_from(char::from(c).to_digit(16).unwrap()).unwrap();
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < b.len() {
            out.push(nib(b[i]) * 16 + nib(b[i + 1]));
            i += 2;
        }
        out
    }

    fn hexlify(b: &[u8]) -> String {
        use std::fmt::Write as _;
        b.iter().fold(String::new(), |mut s, x| {
            write!(s, "{x:02x}").unwrap();
            s
        })
    }

    /// The parse of every V-PP stdin (generated with the pre-commit 4.7.0 oracle by `tests/vectors/gen_v_pp.py`)
    /// equals the vectors: pre-commit's own records for valid UTF-8, the bytes algorithm (D-CRASH-11) otherwise.
    #[test]
    fn pre_push_records_match_v_pp_vectors() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../tests/vectors/v_pp.json")).unwrap();
        let vectors = doc["vectors"].as_array().unwrap();
        assert!(vectors.len() >= 20);
        for v in vectors {
            let stdin = unhex(v["stdin_hex"].as_str().unwrap());
            let mut got: Vec<serde_json::Value> = Vec::new();
            for rec in pre_push_records(&stdin) {
                match rec {
                    Ok(fields) => got.push(serde_json::json!({
                        "fields": fields.iter().map(|f| hexlify(f)).collect::<Vec<_>>()
                    })),
                    Err(e) => {
                        got.push(serde_json::json!({ "error": format!("{}: {e}", e.py_type()) }));
                        break;
                    }
                }
            }
            assert_eq!(serde_json::Value::Array(got), v["snp"], "stdin {stdin:?}");
            if std::str::from_utf8(&stdin).is_ok() {
                assert_eq!(
                    v["snp"], v["pre_commit"],
                    "valid UTF-8 parses exactly like pre-commit"
                );
            }
        }
    }

    fn scripted_git(scripted: Arc<crate::testing::ScriptedBackend>) -> Git {
        Git {
            exe: Arc::new(GitExe {
                argv0: vec![os("/usr/bin/git")],
                caps: crate::GitCaps::for_version(None),
            }),
            env: Env::from_pairs(std::iter::empty::<(OsString, OsString)>()),
            root: Arc::from(Path::new("/")),
            admin_dir: Arc::from(Path::new("/.git")),
            backend: scripted,
            brand: Brand::PreCommit,
        }
    }

    /// `subprocess.check_output` leaves stderr inherited: git's raw bytes reach stderr before the exception (on
    /// success too), and the exception carries no hint.
    #[test]
    fn check_output_forwards_raw_stderr_and_keeps_it_out_of_the_exception() {
        use crate::testing::{Response, ScriptedBackend};
        let scripted = Arc::new(ScriptedBackend::new());
        scripted.on(
            "rev-list *",
            Response::Exit {
                code: 128,
                stdout: Vec::new(),
                stderr: b"fatal: bad object \xff\n".to_vec(),
            },
        );
        scripted.on(
            "rev-parse *",
            Response::Exit {
                code: 0,
                stdout: b"abc\n".to_vec(),
                stderr: b"warning: w\n".to_vec(),
            },
        );
        let git = scripted_git(Arc::clone(&scripted));
        let mut err = Vec::new();
        let e = git
            .check_output_to(&[os("rev-list"), os("x")], &mut err)
            .unwrap_err();
        assert_eq!(err, b"fatal: bad object \xff\n");
        let Failure::Exc { exc, .. } = &e else {
            panic!("{e:?}")
        };
        assert_eq!(exc.hint, None);
        assert_eq!(
            String::from_utf8_lossy(&exc.text),
            "Command '('git', 'rev-list', 'x')' returned non-zero exit status 128."
        );
        let mut err = Vec::new();
        assert_eq!(
            git.check_output_to(&[os("rev-parse"), os("y")], &mut err)
                .unwrap(),
            b"abc\n"
        );
        assert_eq!(err, b"warning: w\n");
    }

    #[test]
    fn combined_rev_parse_shapes() {
        let d = parse_combined(b"../\nfalse\n/r/.git\n/r/.git\n/r/.git/index\n").unwrap();
        assert_eq!(d.cdup, b"../");
        assert_eq!(d.inside, b"false");
        assert_eq!(d.index, b"/r/.git/index");
        let d = parse_combined(b"\nfalse\n/r/.git\n/r/.git\n/r/.git/index\n").unwrap();
        assert_eq!(d.cdup, b"");
        // inside .git: --show-cdup prints nothing
        let d = parse_combined(b"true\n/r/.git\n/r/.git\n/r/.git/index\n").unwrap();
        assert_eq!(
            (d.cdup.as_slice(), d.inside.as_slice()),
            (&b""[..], &b"true"[..])
        );
        // a newline inside a path: ask separately
        assert!(parse_combined(b"\nfalse\n/r/a\nb/.git\n/r/.git\n/r/.git/index\n").is_none());
        assert!(parse_combined(b"\nmaybe\n/r/.git\n/r/.git\n/r/.git/index\n").is_none());
        assert!(parse_combined(b"no newline").is_none());
        assert!(parse_combined(b"").is_none());
    }

    /// `SplitMix64` (the snp-pycompat `tests/properties.rs` precedent): seeded, std-only.
    struct Mix(u64);

    impl Mix {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
        }

        /// A record over `alphabet`, up to `max` bytes.
        fn record(&mut self, alphabet: &[u8], max: usize) -> Vec<u8> {
            let len = self.below(max + 1);
            (0..len)
                .map(|_| alphabet[self.below(alphabet.len())])
                .collect()
        }
    }

    /// The parser before kernel K7 (`split`/`splitn`), kept as the reference of the differential test.
    fn former_parse_numstat_z(out: &[u8]) -> Result<Vec<Vec<u8>>, Vec<u8>> {
        let mut list = Vec::new();
        let mut records = out.split(|&b| b == 0);
        while let Some(rec) = records.next() {
            if rec.is_empty() {
                continue;
            }
            let mut fields = rec.splitn(3, |&b| b == b'\t');
            let (Some(_), Some(_), Some(path)) = (fields.next(), fields.next(), fields.next())
            else {
                return Err(rec.to_vec());
            };
            if path.is_empty() {
                for name in [records.next(), records.next()] {
                    match name {
                        Some(n) if !n.is_empty() => list.push(n.to_vec()),
                        _ => return Err(rec.to_vec()),
                    }
                }
            } else {
                list.push(path.to_vec());
            }
        }
        Ok(list)
    }

    fn check_numstat(out: &[u8]) {
        let new = parse_numstat_z(out);
        match former_parse_numstat_z(out) {
            Ok(names) => {
                let list = new.unwrap_or_else(|e| panic!("{out:?}: {e:?}"));
                assert_eq!(list.iter().collect::<Vec<_>>(), names, "{out:?}");
                assert!(
                    list.iter().all(|p| !p.is_empty() && !p.contains(&0)),
                    "{out:?}"
                );
            }
            Err(rec) => {
                let e = new.expect_err("former parser refused");
                assert_eq!(
                    exc_text(&e),
                    exc_text(&bad_record("apply --numstat -z", &BadRecord(rec))),
                    "{out:?}"
                );
            }
        }
    }

    fn exc_text(f: &Failure) -> (String, String) {
        let Failure::Exc { exc, .. } = f else {
            panic!("{f:?}")
        };
        (
            exc.ty.to_string(),
            String::from_utf8_lossy(&exc.text).into_owned(),
        )
    }

    /// K7 (N1, N2): the verified `numstat_spans` behind `parse_numstat_z` agrees with the former parser on every
    /// output over `{a, TAB, NUL, LF}` up to 8 bytes (87 381 cases) and on seeded random outputs.
    #[test]
    fn numstat_kernel_matches_former_parser() {
        let alphabet = [b'a', b'\t', 0, b'\n'];
        let mut buf = Vec::new();
        for len in 0..=8_u32 {
            for mut code in 0..4_usize.checked_pow(len).unwrap() {
                buf.clear();
                for _ in 0..len {
                    buf.push(alphabet[code % 4]);
                    code /= 4;
                }
                check_numstat(&buf);
            }
        }
        let mut mix = Mix(0x5eed_0007_0000_0001);
        for _ in 0..20_000 {
            let out = mix.record(&[b'1', b'-', b'\t', 0, b'x', b'\n', 0xff], 40);
            check_numstat(&out);
        }
        // git's shape: `<added>\t<deleted>\t<name>\0`, TABs and newlines allowed in names
        assert_eq!(
            parse_numstat_z(b"1\t2\ta\tb\0-\t-\tc\nd\0")
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [&b"a\tb"[..], b"c\nd"]
        );
    }

    /// The chunking loop before kernel K7, kept as the reference of the differential test.
    fn former_chunks(paths: &[&[u8]], budget: usize) -> Vec<Vec<Vec<u8>>> {
        let mut calls = Vec::new();
        let mut args = Vec::new();
        let mut bytes = 0_usize;
        for p in paths {
            let mut spec = b":(literal)".to_vec();
            spec.extend_from_slice(p);
            if bytes > 0 && bytes.saturating_add(spec.len()) > budget {
                calls.push(std::mem::take(&mut args));
                bytes = 0;
            }
            bytes = bytes.saturating_add(spec.len()).saturating_add(1);
            args.push(spec);
        }
        if !paths.is_empty() {
            calls.push(args);
        }
        calls
    }

    /// K7 (N3): `literal_list` equals the former encoder, and `literal_chunks` + `literal_spec` the former chunking
    /// loop, on seeded random lists and budgets (including names longer than the budget).
    #[test]
    fn literal_kernels_match_former_code() {
        let mut mix = Mix(0x5eed_0007_0000_0002);
        for _ in 0..5_000 {
            let n = mix.below(12);
            let records: Vec<Vec<u8>> = (0..n).map(|_| mix.record(b"ab*[\\\n", 30)).collect();
            let paths: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
            let mut former = Vec::new();
            for p in &paths {
                former.extend_from_slice(b":(literal)");
                former.extend_from_slice(p);
                former.push(0);
            }
            assert_eq!(literal_pathspecs(paths.iter().copied()), former);
            let budget = mix.below(80);
            let chunks: Vec<Vec<Vec<u8>>> = pathspec::literal_chunks(&paths, budget)
                .into_iter()
                .map(|(s, e)| {
                    paths[s..e]
                        .iter()
                        .map(|p| pathspec::literal_spec(p))
                        .collect()
                })
                .collect();
            assert_eq!(chunks, former_chunks(&paths, budget), "{paths:?} {budget}");
        }
        assert_eq!(
            former_chunks(&[b"a"], CHECKOUT_CHUNK_BYTES),
            [[b":(literal)a".to_vec()]]
        );
    }

    /// The trusted [`HashSeen`] (ADR-095 §2.6, `proofs/registry.toml` `[[trusted]]`) agrees with the verified
    /// [`pathspec::Scan`] on seeded random record lists, so the rollback list is the same whichever one runs.
    #[test]
    fn seen_set_hash_matches_scan() {
        use pathspec::SeenSet;
        let mut mix = Mix(0x5eed_0007_0000_0003);
        for _ in 0..20_000 {
            let n = mix.below(40);
            let records: Vec<Vec<u8>> = (0..n).map(|_| mix.record(b"ab\0", 3)).collect();
            let records: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
            let hashed = HashSeen.first_seen(&records);
            assert_eq!(hashed, pathspec::Scan.first_seen(&records), "{records:?}");
            assert_eq!(hashed.len(), records.len());
        }
        assert_eq!(
            HashSeen.first_seen(&[b"x", b"y", b"x", b"", b""]),
            [true, true, false, true, false]
        );
    }

    /// K7 (N4): the rollback list is the former first-seen union of the patch's paths and the changed paths, minus
    /// the empty records the former code let through (F3).
    #[test]
    fn rollback_list_is_former_union_without_empty_records() {
        let mut mix = Mix(0x5eed_0007_0000_0004);
        for _ in 0..5_000 {
            let t: Vec<Vec<u8>> = (0..mix.below(25))
                .map(|_| mix.record(b"ab", 2).into_iter().chain(*b"x").collect())
                .collect();
            let c: Vec<Vec<u8>> = (0..mix.below(25)).map(|_| mix.record(b"abx", 2)).collect();
            let t: Vec<&[u8]> = t.iter().map(Vec::as_slice).collect();
            let c: Vec<&[u8]> = c.iter().map(Vec::as_slice).collect();
            let mut seen = std::collections::HashSet::new();
            let former: Vec<&[u8]> = t
                .iter()
                .chain(c.iter().filter(|p| !p.is_empty()))
                .copied()
                .filter(|p| seen.insert(*p))
                .collect();
            assert_eq!(pathspec::rollback_list(&HashSeen, &t, &c), former);
            assert_eq!(pathspec::rollback_list(&pathspec::Scan, &t, &c), former);
        }
    }

    /// F3 (ADR-095 K7, a D-DATA-19 amendment): the checkout sink refuses an empty path (an empty `:(literal)` is the
    /// whole tree) and a NUL-bearing one before running anything, with either checkout form.
    #[test]
    fn checkout_sink_refuses_empty_and_nul_paths() {
        use crate::testing::ScriptedBackend;
        for from_file in [true, false] {
            let scripted = Arc::new(ScriptedBackend::new());
            let mut git = scripted_git(Arc::clone(&scripted));
            let mut caps = crate::GitCaps::for_version(None);
            caps.pathspec_from_file = from_file;
            git.exe = Arc::new(GitExe {
                argv0: vec![os("/usr/bin/git")],
                caps,
            });
            for (bad, shown) in [(&b""[..], "path '':"), (b"a\0b", "path 'a\\x00b':")] {
                let list = ZList::from_records([&b"ok"[..], bad, b"z"]).unwrap();
                let (name, text) = exc_text(&git.checkout_path_calls(&list).unwrap_err());
                assert_eq!(name, "ValueError");
                assert!(text.contains(shown), "{text}");
                assert!(git.checkout_paths(&list, true, Group::Isolated).is_err());
            }
            assert!(scripted.calls().is_empty(), "nothing ran");
            let good = ZList::from_records([&b"a b"[..], b"*"]).unwrap();
            assert_eq!(git.checkout_path_calls(&good).unwrap().len(), 1);
            assert_eq!(git.checkout_path_calls(&ZList::default()).unwrap().len(), 0);
        }
    }

    #[test]
    fn first_seen_union_is_linear_and_keeps_order() {
        let a: Vec<Vec<u8>> = (0..50_000_u32)
            .map(|i| format!("a/{i}").into_bytes())
            .collect();
        let b: Vec<Vec<u8>> = (25_000..75_000_u32)
            .rev()
            .map(|i| format!("a/{i}").into_bytes())
            .collect();
        let t = std::time::Instant::now();
        let u = first_seen_union(a.iter().chain(&b).map(Vec::as_slice)).unwrap();
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(u.len(), 75_000);
        assert_eq!(u.get(0), Some(&b"a/0"[..]));
        assert_eq!(u.get(49_999), Some(&b"a/49999"[..]));
        assert_eq!(u.get(50_000), Some(&b"a/74999"[..]));
        assert_eq!(u.get(74_999), Some(&b"a/50000"[..]));
        let small = first_seen_union([&b"x"[..], b"y", b"x", b"z", b"y"].into_iter()).unwrap();
        assert_eq!(small.iter().collect::<Vec<_>>(), [&b"x"[..], b"y", b"z"]);
    }

    #[test]
    fn helpers() {
        assert_eq!(chomp(b"x\n"), b"x");
        assert_eq!(chomp(b"x\n\n"), b"x\n");
        assert_eq!(stripped(b" a \n"), OsString::from("a"));
        assert_eq!(
            join_norm(Path::new("/r"), b"./x/../.git"),
            PathBuf::from("/r/.git")
        );
        assert_eq!(
            join_norm(Path::new("/r"), b"/abs/.git"),
            PathBuf::from("/abs/.git")
        );
    }

    #[test]
    fn counting_tracks_blankness_and_sink_errors() {
        struct Fail;
        impl io::Write for Fail {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut v = Vec::new();
        let mut c = Counting {
            inner: &mut v,
            written: 0,
            non_ws: false,
            error: None,
        };
        io::Write::write_all(&mut c, b" \n\t").unwrap();
        assert!(!c.non_ws);
        io::Write::write_all(&mut c, b"x").unwrap();
        assert!(c.non_ws);
        assert_eq!(c.written, 4);
        let mut f = Fail;
        let mut c = Counting {
            inner: &mut f,
            written: 0,
            non_ws: false,
            error: None,
        };
        assert!(io::Write::write_all(&mut c, b"x").is_err());
        assert_eq!(c.error, Some(io::ErrorKind::Other));
    }
}
