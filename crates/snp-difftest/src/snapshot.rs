//! Snapshots of every observable effect after a step (10 §7.4.1, K3): the worktree (bytes, exec bit, symlink
//! targets; `.git` excluded), the index (`git ls-files -s`), refs (`for-each-ref`, `HEAD`), the stash list,
//! `.pre-commit-config.yaml` and `.git/config` bytes, the hook scripts in `.git/hooks` (by name and exec bit: the
//! bytes differ by design) and the logical store (the set of `(repo, rev)` entries; the stores differ by FD-2).
//!
//! Owner: WP-35.

use std::collections::BTreeMap;
use std::path::Path;

use crate::materialise::World;

/// One worktree entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Entry {
    /// A regular file: bytes and the executable bit.
    File {
        /// Contents.
        bytes: Vec<u8>,
        /// Any `x` bit set (posix).
        exec: bool,
    },
    /// A symlink and its target.
    Symlink(Vec<u8>),
    /// A directory (recorded so empty directories count).
    Dir,
    /// A FIFO, socket or device.
    Special,
}

/// The state after one step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Worktree entries by path (relative, `/`-separated bytes).
    pub tree: BTreeMap<Vec<u8>, Entry>,
    /// `git ls-files -s` output.
    pub index: Vec<u8>,
    /// `for-each-ref` + `HEAD` + `stash list`.
    pub refs: Vec<u8>,
    /// `.pre-commit-config.yaml` bytes (`None`: absent).
    pub config_file: Option<Vec<u8>>,
    /// `.git/config` bytes.
    pub git_config: Option<Vec<u8>>,
    /// `.git/hooks` entries: name → executable.
    pub hooks: BTreeMap<String, bool>,
    /// Logical store entries (`repo@rev`, sorted).
    pub store: Vec<String>,
}

/// Which parts to capture.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one switch per captured effect"
)]
pub(crate) struct Wanted {
    /// Worktree.
    pub tree: bool,
    /// Index.
    pub index: bool,
    /// Refs and stash.
    pub refs: bool,
    /// Hook scripts.
    pub hooks: bool,
    /// `.git/config`.
    pub git_config: bool,
    /// Logical store.
    pub store: bool,
}

/// Where a side keeps its store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum StoreKind {
    /// pre-commit's SQLite store at `PRE_COMMIT_HOME` (read through the oracle's Python).
    PreCommit,
    /// snp's marker store at `SNP_HOME` (`repos-v1/*/.snp-repo.json`).
    Snp,
}

/// Captures the state of `w` (the work repository is `w.layout.work`).
pub(crate) fn capture(
    w: &mut World,
    want: Wanted,
    store: StoreKind,
    python: Option<&Path>,
) -> Result<State, String> {
    let work = w.layout.work.clone();
    let mut st = State::default();
    if want.tree {
        walk(&work, &work, &mut st.tree)?;
    }
    let is_repo = work.join(".git").exists();
    if is_repo && want.index {
        st.index = w.git(&work, &["ls-files", "-s"])?.stdout;
    }
    if is_repo && want.refs {
        let mut refs = w
            .git(
                &work,
                &["for-each-ref", "--format=%(refname) %(objectname)"],
            )?
            .stdout;
        refs.extend_from_slice(b"HEAD ");
        refs.extend(w.git(&work, &["symbolic-ref", "-q", "HEAD"])?.stdout);
        refs.extend(
            w.git(&work, &["rev-parse", "-q", "--verify", "HEAD"])?
                .stdout,
        );
        refs.extend_from_slice(b"stash:\n");
        refs.extend(w.git(&work, &["stash", "list", "--format=%gd %s"])?.stdout);
        st.refs = refs;
    }
    st.config_file = std::fs::read(work.join(".pre-commit-config.yaml")).ok();
    if want.git_config {
        st.git_config = std::fs::read(work.join(".git/config")).ok();
    }
    if want.hooks
        && let Ok(rd) = std::fs::read_dir(work.join(".git/hooks"))
    {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            st.hooks.insert(name, is_exec(&e.path()));
        }
    }
    if want.store {
        st.store = match store {
            StoreKind::PreCommit => precommit_store(&w.layout.store, python, w)?,
            StoreKind::Snp => snp_store(&w.layout.snp_store),
        };
    }
    Ok(st)
}

fn rel_bytes(base: &Path, p: &Path) -> Vec<u8> {
    let rel = p.trim_prefix(base);
    let mut out = Vec::new();
    for (i, c) in rel.components().enumerate() {
        if i > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(c.as_os_str().as_encoded_bytes());
    }
    out
}

fn is_exec(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _: &Path = p;
        false
    }
}

#[expect(
    clippy::filetype_is_file,
    reason = "FIFOs, sockets and devices are recorded as Special, never read"
)]
fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<Vec<u8>, Entry>) -> Result<(), String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        // An unreadable directory (a scenario's chmod 000) is recorded, not fatal.
        out.insert(rel_bytes(base, dir), Entry::Special);
        return Ok(());
    };
    for e in rd {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = e.path();
        if dir == base && e.file_name() == ".git" {
            continue;
        }
        let ft = e.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
        let rel = rel_bytes(base, &p);
        if ft.is_symlink() {
            let t = std::fs::read_link(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            out.insert(
                rel,
                Entry::Symlink(t.as_os_str().as_encoded_bytes().to_vec()),
            );
        } else if ft.is_dir() {
            out.insert(rel, Entry::Dir);
            walk(base, &p, out)?;
        } else if ft.is_file() {
            // An unreadable file (chmod 000) is recorded by its mode only.
            let bytes = std::fs::read(&p).unwrap_or_else(|_| b"<unreadable>".to_vec());
            out.insert(
                rel,
                Entry::File {
                    bytes,
                    exec: is_exec(&p),
                },
            );
        } else {
            out.insert(rel, Entry::Special);
        }
    }
    Ok(())
}

/// `SELECT repo, ref FROM repos` of pre-commit's `db.db`, through the oracle's own Python (the harness links no
/// SQLite). An absent store is empty.
fn precommit_store(store: &Path, python: Option<&Path>, w: &World) -> Result<Vec<String>, String> {
    let db = store.join("db.db");
    if !db.is_file() {
        return Ok(Vec::new());
    }
    let Some(py) = python else {
        return Ok(vec![String::from(
            "<pre-commit store present; no python to read it>",
        )]);
    };
    let script = "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nfor r,f in sorted(c.execute('SELECT repo, ref FROM repos')):\n    print(f'{r}@{f}')\n";
    let out = crate::exec::run(&crate::exec::Invocation {
        program: py.to_path_buf(),
        args: vec!["-I".into(), "-c".into(), script.into(), db.into_os_string()],
        env: w.env.clone(),
        cwd: w.layout.root.clone(),
        stdin: Vec::new(),
        tty: false,
        timeout: std::time::Duration::from_secs(60),
        signal: None,
    })?;
    if out.exit != Some(0) {
        return Ok(vec![format!(
            "<unreadable pre-commit store: {}>",
            String::from_utf8_lossy(&out.stderr).trim_end()
        )]);
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(normalise_local)
        .collect())
}

/// pre-commit names local repositories `local` / `local:dep1,dep2` with `ref` = the template version; snp's
/// markers carry the same data. Both are rendered `local[:deps]@local`.
fn normalise_local(line: &str) -> String {
    match line.rsplit_once('@') {
        Some((repo, _)) if repo == "local" || repo.starts_with("local:") => format!("{repo}@local"),
        _ => line.to_owned(),
    }
}

/// snp's store: one entry per committed `repos-v1/<key>/.snp-repo.json`.
fn snp_store(store: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(store.join("repos-v1")) else {
        return out;
    };
    for e in rd.flatten() {
        let marker = e.path().join(".snp-repo.json");
        let Ok(bytes) = std::fs::read(&marker) else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            out.push(String::from("<corrupt snp marker>"));
            continue;
        };
        let deps: Vec<String> = v
            .get("deps")
            .and_then(serde_json::Value::as_array)
            .map_or_default(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        let repo = match v.get("kind").and_then(serde_json::Value::as_str) {
            Some("local") => String::from("local"),
            _ => match v.get("repo") {
                // D-DATA-08 keys a local directory by its canonical path, `\`-separated on Windows; the oracle
                // records the config's `repo:` (the scenarios' substituted paths use `/`)
                Some(serde_json::Value::String(s))
                    if cfg!(windows)
                        && v.get("repo_type").and_then(serde_json::Value::as_str)
                            == Some("path") =>
                {
                    s.replace('\\', "/")
                }
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::from("?"),
            },
        };
        let name = if deps.is_empty() {
            repo.clone()
        } else {
            format!("{repo}:{}", deps.join(","))
        };
        let rev = if repo == "local" {
            String::from("local")
        } else {
            v.get("rev")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
                .to_owned()
        };
        out.push(format!("{name}@{rev}"));
    }
    out.sort();
    out
}

/// A readable listing of a tree for diffs: `path [x] <len> bytes` lines plus the contents of text files.
pub(crate) fn render_tree(tree: &BTreeMap<Vec<u8>, Entry>) -> Vec<u8> {
    let mut out = Vec::new();
    for (p, e) in tree {
        out.extend_from_slice(p);
        match e {
            Entry::File { bytes, exec } => {
                out.extend_from_slice(if *exec {
                    b" (file, +x)\n"
                } else {
                    b" (file)\n"
                });
                for line in crate::text::lines_with_ends(bytes) {
                    out.extend_from_slice(b"    | ");
                    out.extend_from_slice(line);
                    if !line.ends_with(b"\n") {
                        out.extend_from_slice(b"\\ no newline\n");
                    }
                }
            }
            Entry::Symlink(t) => {
                out.extend_from_slice(b" -> ");
                out.extend_from_slice(t);
                out.push(b'\n');
            }
            Entry::Dir => out.extend_from_slice(b"/\n"),
            Entry::Special => out.extend_from_slice(b" (special)\n"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_entries_normalise() {
        assert_eq!(
            normalise_local("local@1"),
            "local@1".replace("@1", "@local")
        );
        assert_eq!(normalise_local("local:six@1"), "local:six@local");
        assert_eq!(normalise_local("/r/repos/h@abc"), "/r/repos/h@abc");
    }

    #[test]
    fn snp_markers_are_read() {
        let d = tempfile::tempdir().unwrap();
        let k = d.path().join("repos-v1/abc");
        std::fs::create_dir_all(&k).unwrap();
        std::fs::write(k.join(".snp-repo.json"), br#"{"format":1,"kind":"remote","repo_type":"url","repo":"https://x/y","rev":"v1","deps":["six"]}"#).unwrap();
        let l = d.path().join("repos-v1/loc");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::write(
            l.join(".snp-repo.json"),
            br#"{"format":1,"kind":"local","deps":[]}"#,
        )
        .unwrap();
        assert_eq!(
            snp_store(d.path()),
            vec![
                String::from("https://x/y:six@v1"),
                String::from("local@local")
            ]
        );
    }

    #[test]
    fn walks_trees() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::create_dir_all(d.path().join("sub/empty")).unwrap();
        std::fs::write(d.path().join("sub/a.txt"), b"a\nb").unwrap();
        let mut t = BTreeMap::new();
        walk(d.path(), d.path(), &mut t).unwrap();
        let keys: Vec<&[u8]> = t.keys().map(Vec::as_slice).collect();
        assert_eq!(keys, vec![&b"sub"[..], b"sub/a.txt", b"sub/empty"]);
        let r = render_tree(&t);
        assert!(crate::text::contains_bytes(
            &r,
            b"sub/a.txt (file)\n    | a\n    | b\\ no newline\n"
        ));
    }
}
