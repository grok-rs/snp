//! Per-run cache of the git facts pre-commit-hooks compute, with their exact upstream argv (non `-z`, quotePath and
//! renames honoured: bug-compat) and the hook env (09 §3, §9.6). Used by snp-hooks natives.
//!
//! The cached facts depend only on `HEAD` and the index, so each entry is stamped with the `(size, mtime, inode)` of
//! the index file (`$GIT_INDEX_FILE` or `<git dir>/index`) and of `<git dir>/HEAD` and recomputed when either changed
//! (hooks of other languages may `git add`; 09 §9.6 "Invalidation").
//!
//! Owner: WP-18.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use snp_core::failure::{Failure, pyerr};
use snp_proc::spawn::{Captured, Group, Stdin};
use snp_pycompat::codec::decode_utf8;
use snp_pycompat::repr::{repr_str, repr_str_tuple};
use snp_pycompat::text::{splitlines, strip};

use crate::Git;
use crate::types::parse_ls_files_stage;

/// The identity of one file for cache invalidation: `(len, mtime ns, inode)`; `None` when it does not exist.
type FileStamp = Option<(u64, u128, u64)>;

fn file_stamp(p: &Path) -> FileStamp {
    let md = std::fs::metadata(p).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&md);
    #[cfg(not(unix))]
    let ino = 0;
    Some((md.len(), mtime, ino))
}

/// One cached fact and the stamp it was computed under.
type Slot<T> = Mutex<Option<([FileStamp; 2], T)>>;
/// `(mode, path)` of every index entry.
type Modes = Arc<[(u32, Vec<u8>)]>;

/// Per-run cache of pre-commit-hooks git facts.
#[derive(Debug)]
pub struct PchFacts {
    git: Git,
    index_file: PathBuf,
    head_file: PathBuf,
    added: Slot<Arc<[Vec<u8>]>>,
    tracked: Slot<Arc<[Vec<u8>]>>,
    modes: Slot<Modes>,
    branch: Slot<Option<String>>,
}

/// `repr(bytes)` (pre-commit-hooks' `CalledProcessError` carries stderr as bytes).
fn repr_bytes(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if char::from(c) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(c)),
            c => {
                use std::fmt::Write as _;
                let _infallible: std::fmt::Result = write!(out, "\\x{c:02x}");
            }
        }
    }
    out.push(quote);
    out
}

impl PchFacts {
    /// A cache over `git` (its env is the hook env; 09 §9.6).
    pub fn new(git: Git) -> Self {
        let root = git.root().to_path_buf();
        let index_file = git
            .env()
            .get_nonempty("GIT_INDEX_FILE")
            .map_or_else(|| git.admin_dir.join("index"), |p| root.join(p));
        let head_file = git.admin_dir.join("HEAD");
        Self {
            git,
            index_file,
            head_file,
            added: Mutex::new(None),
            tracked: Mutex::new(None),
            modes: Mutex::new(None),
            branch: Mutex::new(None),
        }
    }

    fn stamp(&self) -> [FileStamp; 2] {
        [file_stamp(&self.index_file), file_stamp(&self.head_file)]
    }

    fn cached<T: Clone>(
        &self,
        slot: &Slot<T>,
        compute: impl FnOnce() -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let mut guard = snp_core::sync::lock(slot);
        let stamp = self.stamp();
        if let Some((s, v)) = guard.as_ref()
            && *s == stamp
        {
            return Ok(v.clone());
        }
        let v = compute()?;
        *guard = Some((stamp, v.clone()));
        Ok(v)
    }

    /// `pre_commit_hooks.util.cmd_output(*cmd)` (`util.py:16-24`): `stdout.decode()` (strict UTF-8; a
    /// `UnicodeDecodeError` comes first), then `retcode` 0 or pre-commit-hooks' own `CalledProcessError`, whose
    /// `str()` is `repr((cmd, retcode, returncode, stdout, stderr))`.
    fn cmd_output(&self, args: &[&str]) -> Result<(i64, String), Failure> {
        let os_args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let c = self.git.run_raw(&os_args)?;
        let stdout = decode_utf8(&c.stdout)
            .map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?
            .to_owned();
        Ok((c.status.0, stdout))
    }

    fn cmd_output_checked(&self, args: &[&str]) -> Result<String, Failure> {
        let os_args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let c = self.git.run_raw(&os_args)?;
        let stdout = decode_utf8(&c.stdout)
            .map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?
            .to_owned();
        if c.status.0 == 0 {
            return Ok(stdout);
        }
        let mut cmd = vec!["git"];
        cmd.extend_from_slice(args);
        Err(Failure::unexpected(
            "CalledProcessError",
            format!(
                "({}, 0, {}, {}, {})",
                repr_str_tuple(&cmd),
                c.status.0,
                repr_str(&stdout),
                repr_bytes(&c.stderr)
            ),
        ))
    }

    fn lines(text: &str) -> Arc<[Vec<u8>]> {
        splitlines(text).map(|l| l.as_bytes().to_vec()).collect()
    }

    /// `diff --staged --name-only --diff-filter=A` (`util.added_files`): C-quoted paths, rename detection per
    /// `diff.renames`, split with `str.splitlines()` (09 §3).
    pub fn added_files(&self) -> Result<Arc<[Vec<u8>]>, Failure> {
        self.cached(&self.added, || {
            let out =
                self.cmd_output_checked(&["diff", "--staged", "--name-only", "--diff-filter=A"])?;
            Ok(Self::lines(&out))
        })
    }

    /// `ls-files` (quoted, non `-z`): `check_case_conflict`'s `git ls-files` (09 §4.E).
    pub fn tracked_files(&self) -> Result<Arc<[Vec<u8>]>, Failure> {
        self.cached(&self.tracked, || {
            let out = self.cmd_output_checked(&["ls-files"])?;
            Ok(Self::lines(&out))
        })
    }

    /// `ls-files --stage` of the whole index as `(mode, path)` (`git ls-files -z --stage`, the record format of the
    /// per-partition `ls-files -z --stage -- <paths>` calls; unmerged paths appear once per stage).
    #[expect(clippy::type_complexity, reason = "frozen §1.4.13 signature")]
    pub fn index_modes(&self) -> Result<Arc<[(u32, Vec<u8>)]>, Failure> {
        self.cached(&self.modes, || {
            let out = self.cmd_output_checked(&["ls-files", "-z", "--stage"])?;
            let entries = parse_ls_files_stage(out.into_bytes()).map_err(|r| {
                Failure::unexpected(
                    "ValueError",
                    format!(
                        "unexpected `git ls-files -z --stage` output: {}",
                        repr_bytes(&r.0)
                    ),
                )
            })?;
            Ok(entries
                .modes
                .iter()
                .zip(entries.paths.iter())
                .map(|(&m, p)| (m, p.to_vec()))
                .collect())
        })
    }

    /// `check-attr filter` (09 §4.E): `check_added_large_files.filter_lfs_files` — `git check-attr filter -z
    /// --stdin` fed `'\0'.join(paths)`, stderr discarded, `check=True` (the stdlib `CalledProcessError`, format B);
    /// the paths whose `filter` is `lfs`, in git's order. No paths: no call.
    pub fn lfs_files(&self, paths: &[&[u8]]) -> Result<Vec<Vec<u8>>, Failure> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let stdin: Vec<u8> = paths.join(&b'\0');
        let args = ["check-attr", "filter", "-z", "--stdin"];
        let mut argv = self.git.exe.argv0.clone();
        argv.extend(args.iter().map(Into::into));
        let c = self
            .git
            .backend
            .capture(
                &argv,
                &self.git.env,
                &self.git.root,
                Stdin::Bytes(Arc::from(stdin)),
                Group::Inherit,
            )
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))?;
        if c.status.0 != 0 {
            let mut bare = vec![OsStr::new("git").to_os_string()];
            bare.extend(args.iter().map(Into::into));
            return Err(Failure::unexpected(
                "CalledProcessError",
                pyerr::called_process_b(&bare, c.status.0),
            ));
        }
        decode_utf8(&c.stdout).map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?;
        let fields: Vec<&[u8]> = snp_pycompat::precommit::zsplit(&c.stdout).collect();
        let mut lfs = Vec::new();
        let mut i = 0_usize;
        while let (Some(name), Some(tag)) = (fields.get(i), fields.get(i.saturating_add(2))) {
            if *tag == b"lfs" {
                lfs.push(name.to_vec());
            }
            i = i.saturating_add(3);
        }
        Ok(lfs)
    }

    /// `symbolic-ref HEAD` (`no_commit_to_branch.is_on_branch`): `None` on a non-zero exit (detached HEAD); else the
    /// branch `'/'.join(ref.strip().split('/')[2:])` (`refs/heads/feature/x` → `feature/x`).
    pub fn current_branch(&self) -> Result<Option<String>, Failure> {
        self.cached(&self.branch, || {
            let (rc, out) = self.cmd_output(&["symbolic-ref", "HEAD"])?;
            if rc != 0 {
                return Ok(None);
            }
            Ok(Some(
                strip(&out).split('/').skip(2).collect::<Vec<_>>().join("/"),
            ))
        })
    }

    /// Any other catalogued call (09 §3: `config core.fileMode`, `ls-files -z --stage -- <paths>`, `status
    /// --porcelain=v2 -z -- <paths>`, `cat-file`, `diff --diff-filter=A --raw …`): `git <args>` with the hook env,
    /// unchecked, never cached.
    pub fn raw(&self, args: &[&OsStr]) -> Result<Captured, Failure> {
        self.git.run_raw(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_repr_like_python() {
        assert_eq!(repr_bytes(b""), "b''");
        assert_eq!(repr_bytes(b"fatal: x\n"), "b'fatal: x\\n'");
        assert_eq!(repr_bytes(b"it's"), "b\"it's\"");
        assert_eq!(repr_bytes(b"'\""), "b'\\'\"'");
        assert_eq!(repr_bytes(b"\x00\xff\\\t\r"), "b'\\x00\\xff\\\\\\t\\r'");
    }
}
