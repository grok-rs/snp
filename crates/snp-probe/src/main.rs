//! Cross-platform probe binary used as a hook entry in tests (10 §7.3.5, 14 §A.6.2, ARCHITECTURE §1.4.8).
//!
//! Every subcommand writes raw bytes, one item per line, and never panics on any input (a usage error prints
//! `snp-probe: <message>` on stderr and exits 2):
//!
//! | subcommand | behaviour |
//! |---|---|
//! | `args A…` | each argument on its own line |
//! | `count A…` | the number of arguments |
//! | `env NAME…` | `NAME=value` or `NAME unset` per name |
//! | `envnames` | every environment variable name, sorted, raw case |
//! | `cwd` | the working directory |
//! | `tty` | `stdin: <bool>` / `stdout: <bool>` / `stderr: <bool>` (Python `isatty()`) |
//! | `stdin-kind` | `file-type: char\|pipe\|disk\|other` and `crt-isatty: 0\|1` |
//! | `exit N` | exit with status N |
//! | `signal N` | die by signal N (POSIX; elsewhere exit 128+N) |
//! | `append TEXT -- FILES…` / `write TEXT -- FILES…` / `truncate -- FILES…` | mutate files |
//! | `bytes HEX` | the decoded bytes on stdout |
//! | `interleave [N]` | N (default 3) alternating `stdout i` / `stderr i` lines, flushed one by one |
//! | `sleep MS [TEXT]` | sleep, then print TEXT (if any) |
//! | `daemonize [--hold] [SECS]` | start a detached copy (new session) sleeping SECS (default 30), print `daemon <pid>`, exit 0; `--hold` keeps stdout/stderr open in the daemon |
//! | `stdin` | copy stdin to stdout |
//! | `fail-on NAME FILES…` | exit 1 if a file argument equals NAME, else print the files |
//! | `ntstatus HEX` | exit with the 32-bit code HEX (Windows NTSTATUS) |
//! | `trap FILE [MS [TIMEOUT]]` | POSIX: create `FILE.ready` once the handlers are installed, append each SIGINT/SIGTERM/SIGHUP name to FILE; MS ms (default 300) after the first signal exit 128+signo; exit 0 after `TIMEOUT_MS` (default 60000) without a signal |
//! | `fill BYTES` | BYTES bytes of the repeating pattern `0123456789abcdef` (output-budget tests) |
//!
//! Owner: WP-15.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::time::Duration;

/// How the probe ends.
enum End {
    /// Exit with this status.
    Code(i32),
    /// Usage error.
    Usage(String),
}

fn main() {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let code = match run(argv.get(1..).unwrap_or_default()) {
        End::Code(c) => c,
        End::Usage(msg) => {
            let _ignored = writeln!(io::stderr(), "snp-probe: {msg}");
            2
        }
    };
    let _ignored = io::stdout().flush();
    #[expect(
        clippy::disallowed_methods,
        reason = "snp-probe is a dev-only test binary: `exit N`/`ntstatus` need arbitrary 32-bit codes, nothing to restore"
    )]
    std::process::exit(code);
}

/// Writes `bytes` to stdout (errors ignored: a closed pipe is the test's business).
fn out(bytes: &[u8]) {
    let mut o = io::stdout().lock();
    let _ignored = o.write_all(bytes).and_then(|()| o.flush());
}

/// Writes one line to stdout.
fn line(s: impl AsRef<[u8]>) {
    let mut v = s.as_ref().to_vec();
    v.push(b'\n');
    out(&v);
}

/// The raw bytes of an argument (`WTF-8` on Windows).
fn raw(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

fn parse_int<T: std::str::FromStr>(s: Option<&OsString>, what: &str) -> Result<T, End> {
    s.and_then(|s| s.to_str())
        .and_then(|s| s.trim().parse::<T>().ok())
        .ok_or_else(|| End::Usage(format!("{what} expects an integer")))
}

fn run(args: &[OsString]) -> End {
    let Some((cmd, rest)) = args.split_first() else {
        return End::Usage("missing subcommand".to_owned());
    };
    let r = match cmd.to_str().unwrap_or("") {
        "args" => {
            for a in rest {
                line(raw(a));
            }
            Ok(0)
        }
        "count" => {
            line(rest.len().to_string());
            Ok(0)
        }
        "env" => {
            for name in rest {
                #[expect(
                    clippy::disallowed_methods,
                    reason = "the probe reports its own environment"
                )]
                let v = std::env::var_os(name);
                let mut l = raw(name).to_vec();
                match v {
                    Some(v) => {
                        l.push(b'=');
                        l.extend_from_slice(raw(&v));
                    }
                    None => l.extend_from_slice(b" unset"),
                }
                line(l);
            }
            Ok(0)
        }
        "envnames" => {
            #[expect(
                clippy::disallowed_methods,
                reason = "the probe reports its own environment"
            )]
            let mut names: Vec<OsString> = std::env::vars_os().map(|(k, _)| k).collect();
            names.sort();
            for n in names {
                line(raw(&n));
            }
            Ok(0)
        }
        "cwd" => std::env::current_dir()
            .map(|d| {
                line(raw(d.as_os_str()));
                0
            })
            .map_err(|e| End::Usage(format!("cwd: {e}"))),
        "tty" => {
            let py = |b: bool| if b { "True" } else { "False" };
            line(format!("stdin: {}", py(isatty::stdin())));
            line(format!("stdout: {}", py(isatty::stdout())));
            line(format!("stderr: {}", py(isatty::stderr())));
            Ok(0)
        }
        "stdin-kind" => {
            let (kind, tty) = isatty::stdin_kind();
            line(format!("file-type: {kind}"));
            line(format!("crt-isatty: {}", u8::from(tty)));
            Ok(0)
        }
        "exit" => parse_int::<i32>(rest.first(), "exit"),
        "ntstatus" => rest
            .first()
            .and_then(|s| s.to_str())
            .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .map(|c| i32::from_ne_bytes(c.to_ne_bytes()))
            .ok_or_else(|| End::Usage("ntstatus expects a hex code".to_owned())),
        "signal" => parse_int::<i32>(rest.first(), "signal").map(die_by_signal),
        "append" | "write" | "truncate" => mutate(cmd.to_str().unwrap_or(""), rest),
        "bytes" => decode_hex(rest.first().map(OsString::as_os_str)).map(|b| {
            out(&b);
            0
        }),
        "interleave" => {
            let n: usize = if rest.is_empty() {
                3
            } else {
                match parse_int(rest.first(), "interleave") {
                    Ok(n) => n,
                    Err(e) => return e,
                }
            };
            for i in 1..=n {
                line(format!("stdout {i}"));
                let mut e = io::stderr().lock();
                let _ignored = writeln!(e, "stderr {i}").and_then(|()| e.flush());
            }
            Ok(0)
        }
        "sleep" => parse_int::<u64>(rest.first(), "sleep").map(|ms| {
            std::thread::sleep(Duration::from_millis(ms));
            if let Some(text) = rest.get(1) {
                line(raw(text));
            }
            0
        }),
        "daemonize" => daemonize(rest),
        "__daemon" => {
            daemon_child(rest);
            Ok(0)
        }
        "stdin" => {
            let mut buf = Vec::new();
            let _ignored = io::stdin().lock().read_to_end(&mut buf);
            out(&buf);
            Ok(0)
        }
        "fail-on" => {
            let Some((name, files)) = rest.split_first() else {
                return End::Usage("fail-on expects NAME".to_owned());
            };
            if files.iter().any(|f| f == name) {
                line(format!("fail-on: found {}", name.to_string_lossy()));
                Ok(1)
            } else {
                for f in files {
                    line(raw(f));
                }
                Ok(0)
            }
        }
        "trap" => trap(rest),
        "fill" => parse_int::<u64>(rest.first(), "fill").map(|n| {
            fill(n);
            0
        }),
        other => Err(End::Usage(format!("unknown subcommand {other:?}"))),
    };
    match r {
        Ok(c) => End::Code(c),
        Err(e) => e,
    }
}

/// `append TEXT -- FILES…`, `write TEXT -- FILES…`, `truncate -- FILES…`.
fn mutate(op: &str, rest: &[OsString]) -> Result<i32, End> {
    let (text, files) = if op == "truncate" {
        (OsStr::new(""), rest)
    } else {
        let Some((t, f)) = rest.split_first() else {
            return Err(End::Usage(format!("{op} expects TEXT")));
        };
        (t.as_os_str(), f)
    };
    let files = match files.split_first() {
        Some((dd, f)) if dd == "--" => f,
        _ => files,
    };
    for f in files {
        let res = match op {
            "append" => std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(f)
                .and_then(|mut h| h.write_all(raw(text))),
            "write" => std::fs::write(f, raw(text)),
            _ => std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(f)
                .map(drop),
        };
        if let Err(e) = res {
            return Err(End::Usage(format!("{op} {}: {e}", f.to_string_lossy())));
        }
    }
    Ok(0)
}

fn decode_hex(s: Option<&OsStr>) -> Result<Vec<u8>, End> {
    let bad = || End::Usage("bytes expects an even-length hex string".to_owned());
    let s = s.and_then(OsStr::to_str).ok_or_else(bad)?;
    let digits: Vec<u8> = s
        .bytes()
        .map(|b| match b {
            b'0'..=b'9' => Some(b.wrapping_sub(b'0')),
            b'a'..=b'f' => Some(b.wrapping_sub(b'a').wrapping_add(10)),
            b'A'..=b'F' => Some(b.wrapping_sub(b'A').wrapping_add(10)),
            _ => None,
        })
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(bad)?;
    if !digits.len().is_multiple_of(2) {
        return Err(bad());
    }
    let mut bytes = Vec::new();
    let mut it = digits.iter();
    while let (Some(h), Some(l)) = (it.next(), it.next()) {
        bytes.push(h.wrapping_shl(4) | l);
    }
    Ok(bytes)
}

/// `n` bytes of `0123456789abcdef…` in 64 KiB writes.
fn fill(n: u64) {
    let block: Vec<u8> = b"0123456789abcdef"
        .iter()
        .copied()
        .cycle()
        .take(64 * 1024)
        .collect();
    let mut o = io::stdout().lock();
    let mut left = n;
    while left > 0 {
        let take = usize::try_from(left.min(64 * 1024)).unwrap_or(0);
        if o.write_all(block.get(..take).unwrap_or_default()).is_err() {
            return;
        }
        left = left.saturating_sub(u64::try_from(take).unwrap_or(left));
    }
    let _ignored = o.flush();
}

/// Dies by `sig` with its default action (POSIX); elsewhere returns 128 + sig.
fn die_by_signal(sig: i32) -> i32 {
    #[cfg(unix)]
    {
        let _ignored = signal_hook::low_level::emulate_default_handler(sig);
    }
    sig.saturating_add(128)
}

/// Starts a detached copy of the probe (`__daemon SECS`) and returns at once.
fn daemonize(rest: &[OsString]) -> Result<i32, End> {
    let hold = rest.first().is_some_and(|a| a == "--hold");
    let secs = rest
        .get(usize::from(hold))
        .map_or(Ok(30), |s| parse_int::<u64>(Some(s), "daemonize"))?;
    let exe = std::env::current_exe().map_err(|e| End::Usage(format!("daemonize: {e}")))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("__daemon").arg(secs.to_string());
    cmd.stdin(std::process::Stdio::null());
    if !hold {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let child = cmd
        .spawn()
        .map_err(|e| End::Usage(format!("daemonize: {e}")))?;
    line(format!("daemon {}", child.id()));
    // Not waited for: the daemon outlives the probe (and is reparented).
    drop(child);
    Ok(0)
}

/// The detached copy: a new session (no controlling terminal, own process group), then sleep.
fn daemon_child(rest: &[OsString]) {
    #[cfg(unix)]
    {
        let _ignored = rustix::process::setsid();
    }
    let secs = parse_int::<u64>(rest.first(), "__daemon").unwrap_or(30);
    std::thread::sleep(Duration::from_secs(secs));
}

/// `trap FILE [MS [TIMEOUT_MS]]`: records SIGINT/SIGTERM/SIGHUP (POSIX).
fn trap(rest: &[OsString]) -> Result<i32, End> {
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let Some(file) = rest.first() else {
            return Err(End::Usage("trap expects FILE".to_owned()));
        };
        let linger = match rest.get(1) {
            Some(ms) => parse_int::<u64>(Some(ms), "trap")?,
            None => 300,
        };
        let timeout = match rest.get(2) {
            Some(ms) => parse_int::<u64>(Some(ms), "trap")?,
            None => 60_000,
        };
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|e| End::Usage(format!("trap: {e}")))?;
        let mut ready = file.clone();
        ready.push(".ready");
        std::fs::write(&ready, b"").map_err(|e| End::Usage(format!("trap: {e}")))?;
        let record = |sig: i32| {
            let name = match sig {
                SIGINT => "INT",
                SIGTERM => "TERM",
                _ => "HUP",
            };
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(file)
            {
                let _ignored = writeln!(f, "{name}");
            }
        };
        let start = std::time::Instant::now();
        let mut first: Option<i32> = None;
        loop {
            for sig in signals.pending() {
                record(sig);
                first = first.or(Some(sig));
            }
            if let Some(sig) = first {
                let until = std::time::Instant::now().checked_add(Duration::from_millis(linger));
                while until.is_some_and(|u| std::time::Instant::now() < u) {
                    std::thread::sleep(Duration::from_millis(5));
                    for later in signals.pending() {
                        record(later);
                    }
                }
                return Ok(sig.saturating_add(128));
            }
            if start.elapsed() > Duration::from_millis(timeout) {
                return Ok(0);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[cfg(not(unix))]
    {
        let _: &[OsString] = rest;
        Err(End::Usage("trap is POSIX-only".to_owned()))
    }
}

/// Python's `isatty()` per stream and the stdin file type.
mod isatty {
    #[cfg(unix)]
    pub(super) fn stdin() -> bool {
        rustix::termios::isatty(std::io::stdin())
    }
    #[cfg(unix)]
    pub(super) fn stdout() -> bool {
        rustix::termios::isatty(std::io::stdout())
    }
    #[cfg(unix)]
    pub(super) fn stderr() -> bool {
        rustix::termios::isatty(std::io::stderr())
    }

    /// `fstat(0)`: `S_ISCHR` → char, `S_ISFIFO` → pipe, `S_ISREG` → disk; `isatty(0)`.
    #[cfg(unix)]
    pub(super) fn stdin_kind() -> (&'static str, bool) {
        use rustix::fs::FileType;
        let kind = match rustix::fs::fstat(std::io::stdin()) {
            Ok(st) => match FileType::from_raw_mode(st.st_mode) {
                FileType::CharacterDevice => "char",
                FileType::Fifo => "pipe",
                FileType::RegularFile => "disk",
                _ => "other",
            },
            Err(_) => "other",
        };
        (kind, stdin())
    }

    /// CRT `_isatty`: a character device (console or `NUL`).
    #[cfg(windows)]
    fn is_char(h: winapi_util::HandleRef) -> bool {
        winapi_util::file::typ(h).is_ok_and(|t| t.is_char())
    }
    #[cfg(windows)]
    pub(super) fn stdin() -> bool {
        is_char(winapi_util::HandleRef::stdin())
    }
    #[cfg(windows)]
    pub(super) fn stdout() -> bool {
        is_char(winapi_util::HandleRef::stdout())
    }
    #[cfg(windows)]
    pub(super) fn stderr() -> bool {
        is_char(winapi_util::HandleRef::stderr())
    }

    /// `GetFileType(GetStdHandle(STD_INPUT_HANDLE))` and CRT `_isatty(0)`.
    #[cfg(windows)]
    pub(super) fn stdin_kind() -> (&'static str, bool) {
        let kind = match winapi_util::file::typ(winapi_util::HandleRef::stdin()) {
            Ok(t) if t.is_char() => "char",
            Ok(t) if t.is_pipe() => "pipe",
            Ok(t) if t.is_disk() => "disk",
            _ => "other",
        };
        (kind, stdin())
    }
}
