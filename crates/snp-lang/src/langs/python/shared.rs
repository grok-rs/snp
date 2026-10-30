//! Helpers shared by the group-1 backends (python, node, golang, rust, ruby; WP-24): the installer environment
//! (`git.no_git_env`), `basic_run_hook` as a [`RunPlan`], the system-toolchain heuristic, verified shared toolchains
//! in `tools-v1` (ARCHITECTURE §6.3) and a `configparser`-compatible reader for the INI files pre-commit's helpers
//! read (`pip.conf`, nodeenv's `[nodeenv]` sections).
//!
//! Owner: WP-24.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_net::archive::{self, Extract};
use snp_net::tools::{self, Layout, Tool};
use snp_net::{Fetch, NetError};
use snp_proc::hookcmd::HookCmdError;

use crate::common::hook_cmd;
use crate::framework::messages;
use crate::{Host, LangError, PlanCtx, RunPlan};

/// A shared toolchain reference of [`crate::InstallOutcome::tools`]: `(tool, version)`.
pub(crate) type ToolRef = (Arc<str>, Arc<str>);

/// `os.pathsep`.
pub(crate) const PATHSEP: &str = if cfg!(windows) { ";" } else { ":" };

/// The variables `no_git_env` keeps besides every non-`GIT_` one (`git.py:27-48`).
const NO_GIT_ENV_KEEP: [&str; 9] = [
    "GIT_EXEC_PATH",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_SSL_CAINFO",
    "GIT_SSL_NO_VERIFY",
    "GIT_CONFIG_COUNT",
    "GIT_HTTP_PROXY_AUTHMETHOD",
    "GIT_ALLOW_PROTOCOL",
    "GIT_ASKPASS",
];

/// `git.no_git_env(env)` (`git.py:27-48`): every `GIT_*` variable is dropped except `GIT_CONFIG_KEY_*`,
/// `GIT_CONFIG_VALUE_*` and the nine allow-listed names.
///
/// pre-commit applies it to golang's `go install` only; snp gives it to every installer it spawns (pip/uv, npm,
/// cargo, gem) so the `GIT_DIR`/`GIT_INDEX_FILE` that git exports to hooks never reach an installer that runs git
/// (D-SEC-05; `setuptools_scm` in linked worktrees). On Windows the names are upper-cased like CPython's `os.environ`
/// copy (ADR-074). The same allow-list as `snp_git::no_git_env`, which snp-lang cannot depend on.
pub(crate) fn no_git_env(env: &Env) -> Env {
    Env::from_pairs(env.iter().filter_map(|(k, v)| {
        let name = match k.to_str() {
            Some(s) if cfg!(windows) => OsString::from(s.to_ascii_uppercase()),
            _ => k.to_os_string(),
        };
        let b = name.as_encoded_bytes();
        let keep = !b.starts_with(b"GIT_")
            || b.starts_with(b"GIT_CONFIG_KEY_")
            || b.starts_with(b"GIT_CONFIG_VALUE_")
            || NO_GIT_ENV_KEEP.iter().any(|k| b == k.as_bytes());
        keep.then(|| (name, v.to_os_string()))
    }))
}

/// `env` with `key` set to `value` (a `dict(os.environ, KEY=value)` copy).
pub(crate) fn with_var(env: &Env, key: &str, value: impl Into<OsString>) -> Env {
    env.patched(&snp_core::env::EnvPatch::default().set(key.to_owned(), value))
}

/// `cmd_output(...)[1].strip()` of a probe's stdout, decoded lossily (D-CRASH-13: pre-commit decodes strictly and
/// crashes on a non-UTF-8 byte).
pub(crate) fn stdout_text(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    snp_pycompat::text::strip(&text).to_owned()
}

/// `lang_base.basic_run_hook` as a plan: `run_xargs(hook_cmd(entry, args), …)` with the activated environment;
/// shuffled and parallel unless the hook sets `require_serial` (decided by the executor).
///
/// An entry that `shlex.split` rejects or that splits to nothing is pre-commit's exit-3 crash; snp fails only that
/// hook (C-hook, X13-EXE-01/02, D-CRASH-05), exactly like the env-less backends.
pub(crate) fn basic_plan(cx: &PlanCtx<'_>) -> RunPlan {
    match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
        Ok(cmd) => RunPlan::Xargs {
            cmd: Arc::from(cmd),
            env: cx.env.clone(),
            shuffle: true,
            serial: false,
            pre: Vec::new(),
        },
        Err(e) => {
            let text = match e {
                HookCmdError::Shlex(err) => messages::shlex_error(err.py_type(), &err.to_string()),
                _ => messages::EMPTY_ENTRY.to_owned(),
            };
            RunPlan::Error {
                code: 1,
                output: text.into_bytes(),
            }
        }
    }
}

/// `lang_base.exe_exists(name)` (`lang_base.py:63-83`), the "system toolchain" heuristic of node, golang and ruby.
///
/// With `SNP_PREFER_SYSTEM_TOOLCHAINS=1` (opt-in, 04a Q17) the `$HOME`/`shims` filter is skipped and any executable on
/// `PATH` counts, so an nvm node or a `~/sdk` go is used instead of a download (the env directory is then named
/// `…-system`, like on a host where the toolchain lives outside `$HOME`).
pub(crate) fn system_exe(h: &Host<'_>, name: &str) -> bool {
    if h.base_env.is_truthy_flag("SNP_PREFER_SYSTEM_TOOLCHAINS") {
        return h
            .probe
            .find_exe(OsStr::new(name), h.base_env, h.cwd)
            .is_some();
    }
    crate::common::exe_exists(OsStr::new(name), h.base_env, h.cwd)
}

/// A filesystem error of `op` on `path`.
pub(crate) fn io_err(op: &'static str, path: &Path, err: io::Error) -> LangError {
    LangError::Io {
        op,
        path: path.to_path_buf(),
        err,
    }
}

/// `os.makedirs(path, exist_ok=True)`.
pub(crate) fn mkdirs(path: &Path) -> Result<(), LangError> {
    std::fs::create_dir_all(path).map_err(|e| io_err("create directory", path, e))
}

/// A [`Failure`] of a tool lock as a backend error (cancellation stays an interruption).
fn lock_failure(path: &Path, f: Failure) -> LangError {
    match f {
        Failure::Interrupted(_) => LangError::Interrupted,
        Failure::Exc { exc, .. } => io_err(
            "lock tool directory",
            path,
            io::Error::other(String::from_utf8_lossy(&exc.text).into_owned()),
        ),
        other => io_err(
            "lock tool directory",
            path,
            io::Error::other(format!("{other:?}")),
        ),
    }
}

/// The fetcher, or snp-net's offline error when the run has none.
pub(crate) fn fetcher<'a>(h: &Host<'a>) -> Result<&'a dyn Fetch, LangError> {
    h.fetch.ok_or(LangError::Net(NetError::Offline))
}

/// Cap on the extracted size of one toolchain (go ≈ 260 MB, node ≈ 200 MB).
const MAX_EXTRACT: u64 = 4 << 30;

/// A shared toolchain directory of `tools-v1` (ARCHITECTURE §6.3), ready to use.
#[derive(Debug, Clone)]
pub(crate) struct SharedTool {
    /// `tools-v1/<tool>/<version>`.
    pub(crate) dir: PathBuf,
    /// The concrete version.
    pub(crate) version: String,
}

/// The shared toolchain `name` (`node`, `go`, `uv`, …) at `requested` (a concrete version or an alias that
/// [`tools::resolve_version`] understands), downloaded, verified and extracted on first use.
///
/// Flow (§6.3): a concrete version whose directory exists is used without the network or a lock; otherwise
/// `tools-v1/<tool>.lock` → re-check → [`tools::resolve`] → [`Fetch::get_verified`] into `tools-v1/<tool>/.dl` →
/// safe two-pass extraction into `<version>.part-<rand>` renamed to `<version>` (so the directory exists only when
/// complete) → the download is removed. `target` is the triple whose build is wanted (usually
/// [`tools::host_target`]).
pub(crate) fn ensure_tool(
    h: &Host<'_>,
    tool: Tool,
    name: &str,
    requested: &str,
    target: &str,
) -> Result<SharedTool, LangError> {
    let concrete = !matches!(requested, "" | "default" | "latest" | "lts");
    if concrete {
        let dir = h.tools.tool_dir(name, requested);
        if dir.is_dir() {
            return Ok(SharedTool {
                dir,
                version: requested.to_owned(),
            });
        }
    }
    let fetch = fetcher(h)?;
    let version = tools::resolve_version(tool, requested, target, fetch)?;
    let dir = h.tools.tool_dir(name, &version);
    if dir.is_dir() {
        return Ok(SharedTool { dir, version });
    }
    let lock_path = dir.parent().unwrap_or(&dir).to_path_buf();
    let _lock = h
        .tools
        .lock_tool(name, h.cancel)
        .map_err(|f| lock_failure(&lock_path, f))?;
    if dir.is_dir() {
        return Ok(SharedTool { dir, version });
    }
    let artifact = tools::resolve(tool, &version, target, fetch)?;
    install_artifact(h, fetch, &artifact, &dir)?;
    Ok(SharedTool { dir, version })
}

/// Downloads `artifact` and unpacks it at `dest` (which must not exist).
pub(crate) fn install_artifact(
    h: &Host<'_>,
    fetch: &dyn Fetch,
    artifact: &tools::Artifact,
    dest: &Path,
) -> Result<(), LangError> {
    let parent = dest.parent().unwrap_or(dest);
    let dl_dir = parent.join(".dl");
    mkdirs(&dl_dir)?;
    let file_name = artifact
        .url
        .rsplit(['/', ':'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let file = fetch.get_verified(
        &artifact.url,
        &artifact.checksum,
        &dl_dir,
        file_name,
        h.cancel,
    )?;
    let result = unpack(&file, &artifact.layout, dest, file_name);
    let _ignored = std::fs::remove_file(&file);
    result
}

/// Unpacks a verified download at `dest`.
fn unpack(file: &Path, layout: &Layout, dest: &Path, file_name: &str) -> Result<(), LangError> {
    let opts = |strip: u8| Extract {
        strip_components: strip,
        allow_symlinks_inside: true,
        max_total_bytes: MAX_EXTRACT,
    };
    match layout {
        Layout::TarGz { strip } => archive::tar_gz(file, dest, &opts(*strip))?,
        Layout::TarXz { strip } => archive::tar_xz(file, dest, &opts(*strip))?,
        Layout::Zip { strip } => archive::zip(file, dest, &opts(*strip))?,
        Layout::Executable => {
            // One executable: `<dest>/<file name>`, made executable, renamed into place as a whole directory.
            let part = dest.with_extension(format!("part-{}", std::process::id()));
            let _ignored = std::fs::remove_dir_all(&part);
            mkdirs(&part)?;
            let target = part.join(file_name);
            std::fs::copy(file, &target).map_err(|e| io_err("copy download", &target, e))?;
            make_executable(&target)?;
            if let Err(e) = std::fs::rename(&part, dest) {
                let _ignored = std::fs::remove_dir_all(&part);
                return Err(io_err("rename download", dest, e));
            }
        }
        _ => {
            return Err(LangError::Net(NetError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported archive layout for {file_name}"),
            ))));
        }
    }
    Ok(())
}

/// `util.make_executable` (`chmod +x`); a no-op on Windows.
pub(crate) fn make_executable(path: &Path) -> Result<(), LangError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let md = std::fs::metadata(path).map_err(|e| io_err("stat", path, e))?;
        let mode = md.permissions().mode() | 0o111;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| io_err("chmod", path, e))?;
    }
    #[cfg(not(unix))]
    {
        let _: &Path = path;
    }
    Ok(())
}

/// A symbolic link `link` → `target` (a directory link on Windows), replacing nothing.
pub(crate) fn symlink(target: &Path, link: &Path, dir: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        let _: bool = dir;
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        if dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _: (&Path, &Path, bool) = (target, link, dir);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symbolic links are not supported",
        ))
    }
}

/// One `configparser` section: `(option, value)` in file order, option names lower-cased (`optionxform`).
pub(crate) type IniSection = Vec<(String, String)>;

/// Cap on an INI file read by a backend.
pub(crate) const MAX_INI: u64 = 1 << 20;

/// A lenient `configparser.ConfigParser().read(...)` of `text`: `[section]` headers, `key = value` / `key: value`
/// options (split at the first `=` or `:`, both sides stripped, the key lower-cased), indented continuation lines
/// appended with `\n`, full-line `#`/`;` comments and blank lines skipped (a blank line ends a value; configparser
/// keeps it and strips it at the end, which yields the same value), no inline comments, no interpolation. A repeated
/// section continues the first one and a later duplicate option overrides the earlier one. Lines outside a section
/// and lines without a delimiter are skipped: configparser raises (`MissingSectionHeaderError`, `ParsingError`,
/// strict-mode duplicates), which crashes nodeenv and makes pip refuse the file; snp never crashes on a user's
/// configuration file.
pub(crate) fn parse_ini(text: &str) -> Vec<(String, IniSection)> {
    let text = snp_pycompat::textio::universal_newlines(text);
    let mut out: Vec<(String, IniSection)> = Vec::new();
    // The section being read.
    let mut cur: Option<usize> = None;
    // Whether the previous line was an option (continuations attach to it).
    let mut in_option = false;
    for line in text.split('\n') {
        let stripped = snp_pycompat::text::strip(line);
        if stripped.is_empty() {
            in_option = false;
            continue;
        }
        if stripped.starts_with('#') || stripped.starts_with(';') {
            continue;
        }
        if line.starts_with([' ', '\t']) && in_option {
            if let Some((_, opts)) = cur.and_then(|i| out.get_mut(i))
                && let Some((_, v)) = opts.last_mut()
            {
                if !v.is_empty() {
                    v.push('\n');
                }
                v.push_str(stripped);
            }
            continue;
        }
        in_option = false;
        if let Some(name) = stripped.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            cur = Some(if let Some(i) = out.iter().position(|(n, _)| n == name) {
                i
            } else {
                out.push((name.to_owned(), Vec::new()));
                out.len().saturating_sub(1)
            });
            continue;
        }
        let Some((_, opts)) = cur.and_then(|i| out.get_mut(i)) else {
            continue;
        };
        let Some(i) = stripped.find(['=', ':']) else {
            continue;
        };
        let key = snp_pycompat::text::strip(stripped.get(..i).unwrap_or_default()).to_lowercase();
        let value =
            snp_pycompat::text::strip(stripped.get(i.saturating_add(1)..).unwrap_or_default())
                .to_owned();
        opts.retain(|(k, _)| *k != key);
        opts.push((key, value));
        in_option = true;
    }
    out
}

/// The options of `section` in a parsed INI file.
pub(crate) fn ini_section<'a>(
    ini: &'a [(String, IniSection)],
    section: &str,
) -> Option<&'a IniSection> {
    ini.iter().find(|(n, _)| n == section).map(|(_, s)| s)
}

/// Reads an INI file (capped, lossy UTF-8); `None` when it cannot be read.
pub(crate) fn read_ini(path: &Path) -> Option<Vec<(String, IniSection)>> {
    let bytes = snp_os::fs::read_capped(path, MAX_INI).ok()?;
    Some(parse_ini(&String::from_utf8_lossy(&bytes)))
}

/// The architecture name pre-commit's golang module uses (`_ARCH`, `golang.py:35-43`) for `target`.
pub(crate) fn go_arch(target: &str) -> &str {
    match target.split('-').next().unwrap_or_default() {
        "x86_64" => "amd64",
        "i686" | "i586" => "386",
        "aarch64" => "arm64",
        "armv7" | "arm" => "armv6l",
        "riscv64gc" => "riscv64",
        "powerpc64le" => "ppc64le",
        "loongarch64" => "loong64",
        other => other,
    }
}

/// `platform.system().lower()` for `target`.
pub(crate) fn os_name(target: &str) -> &'static str {
    if target.contains("apple-darwin") {
        "darwin"
    } else if target.contains("windows") {
        "windows"
    } else if target.contains("freebsd") {
        "freebsd"
    } else {
        "linux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_git_env_keeps_the_allow_list() {
        let e = Env::from_pairs([
            ("GIT_DIR", "x"),
            ("GIT_INDEX_FILE", "y"),
            ("GIT_SSH", "s"),
            ("GIT_CONFIG_KEY_0", "k"),
            ("PATH", "/bin"),
        ]);
        let n = no_git_env(&e);
        assert!(n.get("GIT_DIR").is_none());
        assert!(n.get("GIT_INDEX_FILE").is_none());
        assert_eq!(n.get("GIT_SSH"), Some(OsStr::new("s")));
        assert_eq!(n.get("GIT_CONFIG_KEY_0"), Some(OsStr::new("k")));
        assert_eq!(n.get("PATH"), Some(OsStr::new("/bin")));
    }

    #[test]
    fn ini_parsing_follows_configparser() {
        let ini = parse_ini(
            "# c\n[global]\nindex-url = https://a/simple\nExtra-Index-URL =\n    https://b\n    https://c\n\
             ; x\n[install]\npre: true\n[global]\ntimeout=5\nbad line\n",
        );
        let g = ini_section(&ini, "global").unwrap();
        assert_eq!(
            g,
            &vec![
                ("index-url".to_owned(), "https://a/simple".to_owned()),
                (
                    "extra-index-url".to_owned(),
                    "https://b\nhttps://c".to_owned()
                ),
                ("timeout".to_owned(), "5".to_owned()),
            ]
        );
        let i = ini_section(&ini, "install").unwrap();
        assert_eq!(i, &vec![("pre".to_owned(), "true".to_owned())]);
        assert_eq!(parse_ini("no section = 1\n").len(), 0);
        assert!(
            parse_ini("\r\n\r[x]\rk=v\r")
                .first()
                .is_some_and(|(n, s)| n == "x" && s.len() == 1)
        );
    }

    #[test]
    fn go_names() {
        assert_eq!(go_arch("x86_64-unknown-linux-gnu"), "amd64");
        assert_eq!(go_arch("aarch64-apple-darwin"), "arm64");
        assert_eq!(os_name("aarch64-apple-darwin"), "darwin");
        assert_eq!(os_name("x86_64-pc-windows-msvc"), "windows");
    }
}
