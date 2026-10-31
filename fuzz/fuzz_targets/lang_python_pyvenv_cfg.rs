//! Fuzz target `lang_python_pyvenv_cfg`: `pyvenv.cfg` parsing (and the `pip.conf` reader of the uv installer).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Input: byte 0 selects the parser — even: the bytes are an environment's `pyvenv.cfg` and the python backend's
//! health check runs over it (the fake interpreter answers with the rest of the input, exercising the `version_info`
//! comparison); odd: the bytes are a `pip.conf` (`$PIP_CONFIG_FILE`) and an install runs up to the uv discovery
//! (configuration layering, INI parsing, pip-to-uv translation, the fallback decision). Everything goes through the
//! public `Language` API with a fake runner: nothing is spawned, nothing is downloaded.
//!
//! Owner: WP-24.
#![no_main]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use libfuzzer_sys::fuzz_target;
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::{Printer, PrinterOpts};
use snp_lang::langs::python::Python;
use snp_lang::{CmdRunner, EnvCtx, Host, InstallCtx, LangError, Language, ToolDirs, ToolProbe};
use snp_os::lock::{FileLock, LockMode};
use snp_proc::spawn::{Captured, PyStatus};

/// Answers every command with exit 0 and `stdout`.
#[derive(Debug)]
struct Fake {
    stdout: Vec<u8>,
}

impl CmdRunner for Fake {
    fn run(&self, _argv: &[OsString], _env: &Env, _cwd: &Path) -> Result<Captured, LangError> {
        Ok(Captured {
            status: PyStatus(0),
            stdout: self.stdout.clone(),
            stderr: Vec::new(),
            stopped_by_tty: false,
        })
    }
}

/// Tool directories below the work directory.
#[derive(Debug)]
struct Dirs(PathBuf);

impl ToolDirs for Dirs {
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        self.0.join("tools").join(tool).join(version)
    }

    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<FileLock, Failure> {
        let dir = self.0.join("tools");
        std::fs::create_dir_all(&dir).map_err(|e| Failure::from_io(&e, None, None))?;
        FileLock::acquire(&dir.join(format!("{tool}.lock")), LockMode::Exclusive, &mut || {}, c, None)
            .map_err(|e| Failure::from_io(&e, None, None))
    }

    fn cache_dir(&self, name: &str) -> PathBuf {
        self.0.join("cache").join(name)
    }
}

/// A fresh work directory per input (removed afterwards).
fn workdir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("snp-fuzz-pyvenv-{}-{n}", std::process::id()))
}

fn one(data: &[u8], dir: &Path) {
    let Some((&sel, body)) = data.split_first() else {
        return;
    };
    let prefix = dir.join("prefix");
    let env_dir = prefix.join("py_env-python3.12");
    if std::fs::create_dir_all(env_dir.join("bin")).is_err() {
        return;
    }
    let conf = dir.join("pip.conf");
    let (cfg_bytes, answer) = body.split_at(body.len() / 2);
    let _ = std::fs::write(env_dir.join("pyvenv.cfg"), if sel % 2 == 0 { body } else { cfg_bytes });
    let _ = std::fs::write(&conf, body);
    let env = Env::from_pairs([
        ("PATH", dir.join("nothing").into_os_string()),
        ("HOME", dir.join("home").into_os_string()),
        ("PIP_CONFIG_FILE", conf.into_os_string()),
        ("XDG_CONFIG_DIRS", dir.join("xdg").into_os_string()),
        ("SNP_UV", dir.join("uv").into_os_string()),
        ("SNP_PYTHON_DOWNLOADS", OsString::from("never")),
    ]);
    let cancel = CancelToken::new();
    let printer = Printer::new(
        Box::new(std::io::sink()),
        Box::new(std::io::sink()),
        PrinterOpts {
            brand: Brand::Snp,
            color: false,
            stdout_is_tty: false,
            windows_text_mode: false,
            cancel: cancel.clone(),
        },
    );
    let probe = ToolProbe::new(None);
    let tools = Dirs(dir.to_path_buf());
    let runner = Fake {
        stdout: answer.to_vec(),
    };
    let self_exe = dir.join("snp");
    let host = Host {
        base_env: &env,
        cwd: dir,
        self_exe: &self_exe,
        probe: &probe,
        fetch: None,
        tools: &tools,
        runner: &runner,
        printer: &printer,
        cancel: &cancel,
        brand: Brand::Snp,
    };
    let cx = EnvCtx {
        host: &host,
        prefix: &prefix,
        version: "python3.12",
        env_dir: &env_dir,
    };
    if sel % 2 == 0 {
        let _ = Python.health(&cx, None);
    } else {
        let deps: Vec<Arc<str>> = vec![Arc::from("six")];
        let fresh = prefix.join("py_env-new");
        let _ = Python.install(&InstallCtx {
            cx: EnvCtx {
                env_dir: &fresh,
                ..cx
            },
            deps: &deps,
            is_local: false,
        });
        let _ = Python.health(
            &EnvCtx {
                host: &host,
                prefix: &prefix,
                version: "python3.12",
                env_dir: &env_dir,
            },
            None,
        );
    }
}

fuzz_target!(|data: &[u8]| {
    let dir = workdir();
    if std::fs::create_dir_all(&dir).is_ok() {
        one(data, &dir);
    }
    let _ = std::fs::remove_dir_all(&dir);
});
