//! Helpers shared by the group-1 backends (python, node, golang, rust, ruby; WP-24): the installer environment
//! (`git.no_git_env`), `basic_run_hook` as a [`RunPlan`], the system-toolchain heuristic, verified shared toolchains
//! in `tools-v1` (ARCHITECTURE §6.3) and a `configparser`-compatible reader for the INI files pre-commit's helpers
//! read (`pip.conf`, nodeenv's `[nodeenv]` sections).
//!
//! Owner: WP-24.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, RandomState};
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
/// pre-commit applies it to golang's `go install` and (since 4.7.0) node's `npm install`; snp gives it to every
/// installer it spawns (also pip/uv, cargo, gem) so the `GIT_DIR`/`GIT_INDEX_FILE` that git exports to hooks never
/// reach an installer that runs git (D-SEC-05; `setuptools_scm` in linked worktrees). On Windows the names are upper-cased like CPython's `os.environ`
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
    if is_version_component(requested) {
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

/// Whether `requested` can name a `tools-v1/<tool>/<version>` directory as it is: a concrete version made of
/// `[0-9A-Za-z._+-]` (at most 64 bytes, snp-net's version alphabet), not an alias and not `.`/`..`. Anything else
/// (`../../usr`, a path, a range) goes through [`tools::resolve_version`], which rejects what it does not know, so a
/// `language_version` or `.node-version` value never selects a directory outside the shared tools.
pub(crate) fn is_version_component(requested: &str) -> bool {
    !matches!(requested, "" | "." | ".." | "default" | "latest" | "lts")
        && requested.len() <= 64
        && requested
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_+".contains(&b))
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
            let part = part_dir(dest)?;
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

/// A fresh, empty `<dest>.part-<rand>` directory next to `dest` (the version is kept whole: `1.28.2` becomes
/// `1.28.2.part-…`, the name snp-net's archive extraction and the store's sweep use).
fn part_dir(dest: &Path) -> Result<PathBuf, LangError> {
    let name = dest.file_name().unwrap_or_else(|| OsStr::new("tool"));
    let parent = dest.parent().unwrap_or(dest);
    let mut last = None;
    for attempt in 0_u32..8 {
        // `RandomState` has per-process random keys; the clock and the attempt vary the value within a process.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let rand = RandomState::new().hash_one((std::process::id(), nanos, attempt));
        let mut n = name.to_os_string();
        n.push(format!(".part-{rand:016x}"));
        let part = parent.join(n);
        #[expect(
            clippy::create_dir,
            reason = "the directory must be new: an existing one is a name collision and retried"
        )]
        let created = std::fs::create_dir(&part);
        match created {
            Ok(()) => return Ok(part),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some((part, e)),
            Err(e) => return Err(io_err("create directory", &part, e)),
        }
    }
    let (part, e) = last.unwrap_or_else(|| (dest.to_path_buf(), io::Error::other("no name")));
    Err(io_err("create directory", &part, e))
}

/// `util.make_executable` (`chmod +x`); a no-op on Windows.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "fallible on Unix, a no-op elsewhere"
    )
)]
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

/// One `configparser` section: `(option, value)` in file order (option names lower-cased, `optionxform`), with an
/// index used only for lookups (never iterated, so the file order stays the only order; S11).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct IniSection {
    /// `(option, value)` in the order the options first appeared.
    opts: Vec<(String, String)>,
    /// option → position in `opts`.
    index: HashMap<String, usize>,
}

impl IniSection {
    /// The value of `option` defined in this section itself.
    fn get(&self, option: &str) -> Option<&str> {
        self.index
            .get(option)
            .and_then(|&i| self.opts.get(i))
            .map(|(_, v)| v.as_str())
    }
}

/// A parsed INI file: its sections in file order (without `DEFAULT`) and the `DEFAULT` section.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Ini {
    /// `(name, options)` in the order the sections first appeared.
    sections: Vec<(String, IniSection)>,
    /// section name → position in `sections`.
    index: HashMap<String, usize>,
    /// `[DEFAULT]`, merged into every section by [`Ini::items`].
    defaults: IniSection,
}

impl Ini {
    /// `parser.sections()`.
    pub(crate) fn sections(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|(n, _)| n.as_str())
    }

    /// `parser.items(section)` of `RawConfigParser`: the `DEFAULT` options (in their order, overridden by the
    /// section's values), then the section's own other options; `None` when the section does not exist
    /// (`NoSectionError`; `DEFAULT` itself is not a section).
    pub(crate) fn items(&self, section: &str) -> Option<Vec<(&str, &str)>> {
        let own = &self.sections.get(*self.index.get(section)?)?.1;
        let mut out: Vec<(&str, &str)> = Vec::new();
        for (k, v) in &self.defaults.opts {
            out.push((k.as_str(), own.get(k).unwrap_or(v.as_str())));
        }
        for (k, v) in &own.opts {
            if !self.defaults.index.contains_key(k) {
                out.push((k.as_str(), v.as_str()));
            }
        }
        Some(out)
    }

    /// `parser.has_option(section, option)` and `parser.get(section, option)` (raw: no interpolation).
    pub(crate) fn get(&self, section: &str, option: &str) -> Option<&str> {
        let own = &self.sections.get(*self.index.get(section)?)?.1;
        own.get(option).or_else(|| self.defaults.get(option))
    }
}

/// Cap on an INI file read by a backend.
pub(crate) const MAX_INI: u64 = 1 << 20;

/// A section being read: `(option, value lines)` plus the lookup index.
#[derive(Default)]
struct RawSection {
    /// `(option, lines)` in first-appearance order.
    opts: Vec<(String, Vec<String>)>,
    /// option → position.
    index: HashMap<String, usize>,
}

impl RawSection {
    /// `cursect[optname] = [optval]`: a repeated option keeps its first position (a `dict`); returns its position.
    fn set(&mut self, name: String, value: String) -> usize {
        if let Some(&i) = self.index.get(&name) {
            if let Some(slot) = self.opts.get_mut(i) {
                slot.1 = vec![value];
            }
            return i;
        }
        let i = self.opts.len();
        self.index.insert(name.clone(), i);
        self.opts.push((name, vec![value]));
        i
    }

    /// `_join_multiline_values`: `'\n'.join(lines).rstrip()`.
    fn finish(self) -> IniSection {
        let opts = self
            .opts
            .into_iter()
            .map(|(k, lines)| {
                let joined = lines.join("\n");
                let v = snp_pycompat::text::rstrip(&joined).to_owned();
                (k, v)
            })
            .collect();
        IniSection {
            opts,
            index: self.index,
        }
    }
}

/// `configparser.RawConfigParser(strict=False).read(path)` over `text` (CPython 3.12/3.13 `_read`): lines split at
/// `\n` after universal-newline translation; `#`/`;` full-line comments (after stripping) are skipped without ending
/// a value; an empty line is kept inside the current value (`empty_lines_in_values`); a line indented deeper than
/// the line that opened the current option continues it; a `[header]` is matched as a prefix (`\[(?P<header>.+)\]`
/// on the stripped line, so `[install] ; x` opens `install`); `name = value` / `name: value` splits at the first
/// delimiter, the name right-stripped and lower-cased, the value stripped; a repeated section continues the first
/// one and a repeated option overrides the earlier value at its first position; `[DEFAULT]` options apply to every
/// section ([`Ini::items`]); values are joined with `\n` and right-stripped. No inline comments, no interpolation.
///
/// Where configparser raises — options before any section header (`MissingSectionHeaderError`, which stops
/// reading), lines without a delimiter (`ParsingError`, raised after the whole file was read) and strict-mode
/// duplicates — snp is lenient: the offending line is ignored and reading goes on, so the result is the parser's
/// state as if the error had not been raised. pip refuses such a file and nodeenv crashes; snp never crashes on a
/// user's configuration file. Every lookup is hashed: the parse is linear in the file size.
pub(crate) fn parse_ini(text: &str) -> Ini {
    let text = snp_pycompat::textio::universal_newlines(text);
    // Position 0 is `DEFAULT`.
    let mut raw: Vec<(String, RawSection)> = vec![(String::from("DEFAULT"), RawSection::default())];
    let mut sect_index: HashMap<String, usize> = HashMap::new();
    let mut cursect: Option<usize> = None;
    // The current option (`optname` when non-empty) as a position in the current section.
    let mut optname: Option<usize> = None;
    let mut indent_level: usize = 0;
    for line in text.split_inclusive('\n') {
        let stripped = snp_pycompat::text::strip(line);
        let comment = stripped.starts_with('#') || stripped.starts_with(';');
        let value = if comment { "" } else { stripped };
        if value.is_empty() {
            // `empty_lines_in_values`: an empty line (not a comment) is part of the current value.
            if !comment
                && let (Some(s), Some(o)) = (cursect, optname)
                && let Some((_, lines)) = raw.get_mut(s).and_then(|(_, r)| r.opts.get_mut(o))
            {
                lines.push(String::new());
            }
            continue;
        }
        let cur_indent_level = line
            .chars()
            .position(|c| !snp_pycompat::chars::is_space(c))
            .unwrap_or(0);
        if let (Some(s), Some(o)) = (cursect, optname)
            && cur_indent_level > indent_level
        {
            if let Some((_, lines)) = raw.get_mut(s).and_then(|(_, r)| r.opts.get_mut(o)) {
                lines.push(value.to_owned());
            }
            continue;
        }
        indent_level = cur_indent_level;
        if let Some(header) = section_header(value) {
            let i = if header == "DEFAULT" {
                0
            } else if let Some(&i) = sect_index.get(header) {
                i
            } else {
                let i = raw.len();
                sect_index.insert(header.to_owned(), i);
                raw.push((header.to_owned(), RawSection::default()));
                i
            };
            cursect = Some(i);
            optname = None;
            continue;
        }
        let Some(s) = cursect else {
            // `MissingSectionHeaderError`.
            continue;
        };
        let Some(d) = value.find(['=', ':']) else {
            // `ParsingError` (raised at the end): the line is ignored, `optname` stays.
            continue;
        };
        let name = snp_pycompat::text::rstrip(value.get(..d).unwrap_or_default()).to_lowercase();
        let val = snp_pycompat::text::strip(value.get(d.saturating_add(1)..).unwrap_or_default())
            .to_owned();
        let empty = name.is_empty();
        let pos = raw.get_mut(s).map(|(_, r)| r.set(name, val));
        // An empty option name is stored but is falsy: nothing continues it.
        optname = if empty { None } else { pos };
    }
    let mut it = raw.into_iter();
    let defaults = it.next().map(|(_, r)| r.finish()).unwrap_or_default();
    let mut sections: Vec<(String, IniSection)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (name, r) in it {
        index.insert(name.clone(), sections.len());
        sections.push((name, r.finish()));
    }
    Ini {
        sections,
        index,
        defaults,
    }
}

/// `SECTCRE.match(value)`: `\[(?P<header>.+)\]` at the start of the stripped line, the header running to the last
/// `]` (greedy) and non-empty.
fn section_header(value: &str) -> Option<&str> {
    let rest = value.strip_prefix('[')?;
    let end = rest.rfind(']')?;
    let header = rest.get(..end)?;
    (!header.is_empty()).then_some(header)
}

/// Reads an INI file (capped, lossy UTF-8); `None` when it cannot be read.
pub(crate) fn read_ini(path: &Path) -> Option<Ini> {
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

    fn items(ini: &Ini, section: &str) -> Vec<(String, String)> {
        ini.items(section)
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn ini_parsing_follows_configparser() {
        let ini = parse_ini(
            "# c\n[global]\nindex-url = https://a/simple\nExtra-Index-URL =\n    https://b\n    https://c\n\
             ; x\n[install]\npre: true\n[global]\ntimeout=5\nbad line\n",
        );
        assert_eq!(
            items(&ini, "global"),
            pairs(&[
                ("index-url", "https://a/simple"),
                ("extra-index-url", "\nhttps://b\nhttps://c"),
                ("timeout", "5"),
            ])
        );
        assert_eq!(items(&ini, "install"), pairs(&[("pre", "true")]));
        assert_eq!(parse_ini("no section = 1\n").sections().count(), 0);
        let ini = parse_ini("\r\n\r[x]\rk=v\r");
        assert_eq!(items(&ini, "x"), pairs(&[("k", "v")]));
    }

    /// The reviewer's cases: indented options are options, not continuations; a blank line stays inside a
    /// multi-line value; a header is a prefix match; `[DEFAULT]` applies to every section.
    #[test]
    fn ini_indentation_blank_lines_headers_and_defaults() {
        let ini = parse_ini("[global]\n  index-url = https://corp/simple\n  trusted-host = corp\n");
        assert_eq!(
            items(&ini, "global"),
            pairs(&[
                ("index-url", "https://corp/simple"),
                ("trusted-host", "corp")
            ])
        );
        let ini = parse_ini("[global]\nextra-index-url =\n    https://a\n\n    https://b\n");
        assert_eq!(
            items(&ini, "global"),
            pairs(&[("extra-index-url", "\nhttps://a\n\nhttps://b")])
        );
        let ini = parse_ini("[global]\na = 1\n[install] ; corp\nb = 2\n");
        assert_eq!(items(&ini, "install"), pairs(&[("b", "2")]));
        assert_eq!(items(&ini, "global"), pairs(&[("a", "1")]));
        let ini = parse_ini("[DEFAULT]\nx = d\ny = d\n[s]\ny = s\nz = s\n");
        assert_eq!(
            items(&ini, "s"),
            pairs(&[("x", "d"), ("y", "s"), ("z", "s")])
        );
        assert_eq!(ini.get("s", "x"), Some("d"));
        assert!(ini.items("DEFAULT").is_none());
        // A deeper-indented line after an error line continues the option before it; a comment does not end it.
        let ini = parse_ini("[s]\n  a = 1\njunk\n # c\n x\n");
        assert_eq!(items(&ini, "s"), pairs(&[("a", "1\nx")]));
        // A repeated option keeps its first position.
        let ini = parse_ini("[s]\na = 1\nb = 2\na = 3\n");
        assert_eq!(items(&ini, "s"), pairs(&[("a", "3"), ("b", "2")]));
    }

    /// Linear time: eight times the keys and section headers take about eight times as long, against 64 times for a
    /// quadratic parser. The sizes are compared with each other, on one machine, so a slow runner cannot fail this.
    #[test]
    fn ini_parsing_is_linear() {
        use std::fmt::Write as _;
        let ini_of = |n: usize| {
            let mut text = String::from("[global]\n");
            for i in 0..n {
                writeln!(text, "k{i} = v").unwrap();
            }
            for i in 0..n {
                writeln!(text, "[s{i}]\nk = v").unwrap();
            }
            text
        };
        // The fastest of three: noise only ever adds time.
        let parse_time = |n: usize| {
            let text = ini_of(n);
            (0..3)
                .map(|_| {
                    let start = std::time::Instant::now();
                    let ini = parse_ini(&text);
                    let elapsed = start.elapsed();
                    assert_eq!(ini.items("global").map(|v| v.len()), Some(n));
                    assert_eq!(ini.sections().count(), n + 1);
                    elapsed
                })
                .min()
                .unwrap()
        };
        let (small, large) = (parse_time(12_500), parse_time(100_000));
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(
            ratio < 24.0,
            "{small:?} -> {large:?}: {ratio:.1}x for 8x the input"
        );
    }

    /// Differential against CPython's `RawConfigParser(strict=False)` (when `python3.12`/`python3.13` exists):
    /// random files built from the atoms that matter (indentation, blank lines, comments, headers, delimiters,
    /// `DEFAULT`, CR line ends).
    #[test]
    fn ini_matches_cpython_configparser() {
        const ATOMS: [&str; 24] = [
            "[global]",
            "[install]",
            "[DEFAULT]",
            "[install] ; c",
            "[a]b]",
            "[]",
            "  ",
            "\t",
            "key = v",
            "Key: v",
            "k=",
            "x = 1 = 2",
            "# c",
            "; c",
            "  # c",
            "",
            "junk",
            "  cont",
            "    deeper",
            "\tk2 = t",
            "=v",
            " : v",
            "\r",
            "é = ü",
        ];
        let Some(py) = ["/usr/bin/python3.12", "/usr/bin/python3.13"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.is_file())
        else {
            return;
        };
        let d = tempfile::tempdir().unwrap();
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut inputs: Vec<String> = Vec::new();
        for _ in 0..2000 {
            let mut t = String::from("[global]\n");
            let n = next() % 16;
            for _ in 0..n {
                let a = ATOMS[usize::try_from(next() % 24).unwrap_or(0)];
                t.push_str(a);
                t.push('\n');
            }
            inputs.push(t);
        }
        let mut files = Vec::new();
        for (i, t) in inputs.iter().enumerate() {
            let f = d.path().join(format!("{i}.ini"));
            std::fs::write(&f, t).unwrap();
            files.push(f);
        }
        let script = "import configparser, json, sys\n\
            out = []\n\
            for f in sys.argv[1:]:\n\
            \x20   p = configparser.RawConfigParser(strict=False)\n\
            \x20   try:\n\
            \x20       p.read(f, encoding='utf-8')\n\
            \x20   except configparser.Error:\n\
            \x20       pass\n\
            \x20   out.append([[s, [list(kv) for kv in p.items(s)]] for s in p.sections()])\n\
            print(json.dumps(out))\n";
        let out = std::process::Command::new(py)
            .arg("-I")
            .arg("-c")
            .arg(script)
            .args(&files)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let want: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        for (i, t) in inputs.iter().enumerate() {
            let ini = parse_ini(t);
            let got: Vec<serde_json::Value> = ini
                .sections()
                .map(|s| serde_json::json!([s, items(&ini, s)]))
                .collect();
            assert_eq!(serde_json::Value::Array(got), want[i], "input {t:?}");
        }
    }

    /// A single-executable download is staged in `<version>.part-<16 hex>` (the version kept whole), fresh each time.
    #[test]
    fn part_directories_keep_the_version() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("1.28.2");
        let a = part_dir(&dest).unwrap();
        let b = part_dir(&dest).unwrap();
        assert_ne!(a, b);
        for p in [&a, &b] {
            let name = p.file_name().unwrap().to_str().unwrap();
            let hex = name.strip_prefix("1.28.2.part-").unwrap();
            assert_eq!(hex.len(), 16, "{name}");
            assert!(p.is_dir() && std::fs::read_dir(p).unwrap().next().is_none());
        }
    }

    #[test]
    fn version_components() {
        for ok in [
            "20.11.1",
            "1.28.2",
            "3.2.2",
            "1.79.0",
            "go1.22",
            "v20.0.0+x",
        ] {
            assert!(is_version_component(ok), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            "../../usr",
            "a/b",
            "a\\b",
            "default",
            "latest",
            "lts",
            "20 x",
        ] {
            assert!(!is_version_component(bad), "{bad}");
        }
        assert!(!is_version_component(&format!("{:9>65}", "")));
    }

    #[test]
    fn go_names() {
        assert_eq!(go_arch("x86_64-unknown-linux-gnu"), "amd64");
        assert_eq!(go_arch("aarch64-apple-darwin"), "arm64");
        assert_eq!(os_name("aarch64-apple-darwin"), "darwin");
        assert_eq!(os_name("x86_64-pc-windows-msvc"), "windows");
    }
}
