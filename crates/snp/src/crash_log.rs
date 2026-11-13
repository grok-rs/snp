//! The crash log (ARCHITECTURE §10.3, ADR-013): pre-commit's skeleton (`error_handler.py:30-68`) with snp's
//! version lines, the error source chain and, for panics, the Rust backtrace and the bug-report hint. Never contains
//! environment variables.
//!
//! Owner: WP-33.

use std::path::Path;

use snp_core::brand::Brand;
use snp_core::failure::PyExc;
use snp_core::profile;

/// The toolchain the release is built with (`rust-toolchain.toml`).
const RUSTC: &str = "1.100.0";

/// What the version block names.
#[derive(Debug)]
pub(crate) struct LogInfo<'a> {
    /// The brand (`pre-commit version: 4.7.0` | `snp version: 2.0.1`).
    pub(crate) brand: Brand,
    /// `git --version`'s line, when git was resolved during the command.
    pub(crate) git_version: Option<&'a str>,
    /// `sys.executable`: snp's own path.
    pub(crate) executable: &'a Path,
}

/// `sys.platform`.
const fn platform() -> &'static str {
    match std::env::consts::OS.as_bytes() {
        b"linux" => "linux",
        b"macos" => "darwin",
        b"windows" => "win32",
        _ => std::env::consts::OS,
    }
}

/// `os.name`.
const fn os_name() -> &'static str {
    if cfg!(windows) { "nt" } else { "posix" }
}

fn line(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(b'\n');
}

/// The log body: `### version information` … `### error information` with `line1` and the details block.
pub(crate) fn body(info: &LogInfo<'_>, line1: &[u8], exc: Option<&PyExc>) -> Vec<u8> {
    let mut out = Vec::new();
    line(&mut out, "### version information");
    line(&mut out, "");
    line(&mut out, "```");
    match info.brand {
        Brand::PreCommit => line(
            &mut out,
            &format!("pre-commit version: {}", profile::PRE_COMMIT),
        ),
        Brand::Snp => line(&mut out, &format!("snp version: {}", profile::SNP_VERSION)),
    }
    line(
        &mut out,
        &format!("git --version: {}", info.git_version.unwrap_or("(not run)")),
    );
    line(&mut out, "sys.version:");
    line(
        &mut out,
        &format!(
            "    snp {} (rustc {RUSTC}) [{}-{}]",
            profile::SNP_VERSION,
            std::env::consts::ARCH,
            std::env::consts::OS
        ),
    );
    line(
        &mut out,
        &format!("sys.executable: {}", info.executable.display()),
    );
    line(&mut out, &format!("os.name: {}", os_name()));
    line(&mut out, &format!("sys.platform: {}", platform()));
    line(&mut out, "```");
    line(&mut out, "");
    line(&mut out, "### error information");
    line(&mut out, "");
    line(&mut out, "```");
    out.extend_from_slice(line1);
    out.push(b'\n');
    line(&mut out, "```");
    line(&mut out, "");
    line(&mut out, "```");
    let mut details: Vec<String> = Vec::new();
    if let Some(exc) = exc {
        details.extend(exc.chain.iter().cloned());
        if let Some(bt) = &exc.backtrace {
            details.push(String::from("stack backtrace:"));
            details.extend(bt.lines().map(str::to_owned));
        }
        if exc.ty == "PanicException" {
            details.push(format!(
                "snp panicked: this is a bug. Please report it at {} with this log.",
                profile::BUG_URL
            ));
        }
    }
    if details.is_empty() {
        details.push(String::from("(no further details)"));
    }
    let joined = details.join("\n");
    line(&mut out, joined.trim_end());
    line(&mut out, "```");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version block names the pinned toolchain (the published crate has no `rust-toolchain.toml`: skipped).
    #[test]
    fn rustc_is_the_pinned_toolchain() {
        let pin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml");
        let Ok(text) = std::fs::read_to_string(&pin) else {
            return;
        };
        let channel = format!("channel = \"{RUSTC}\"");
        assert!(
            text.lines().any(|l| l.trim() == channel),
            "crash_log::RUSTC ({RUSTC}) is not the channel of {}",
            pin.display()
        );
    }

    #[test]
    fn skeleton() {
        let info = LogInfo {
            brand: Brand::PreCommit,
            git_version: Some("git version 2.43.0"),
            executable: Path::new("/usr/bin/snp"),
        };
        let exc = PyExc::new("CalledProcessError", "x");
        let b = String::from_utf8(body(
            &info,
            b"An unexpected error has occurred: CalledProcessError: x",
            Some(&exc),
        ))
        .unwrap();
        assert!(b.starts_with("### version information\n\n```\npre-commit version: 4.7.0\ngit --version: git version 2.43.0\nsys.version:\n    snp "));
        assert!(b.contains("\nsys.executable: /usr/bin/snp\n"));
        assert!(b.contains("```\n\n### error information\n\n```\nAn unexpected error has occurred: CalledProcessError: x\n```\n\n```\n"));
        assert!(b.ends_with("```\n"));
        let info = LogInfo {
            brand: Brand::Snp,
            git_version: None,
            executable: Path::new("/x"),
        };
        let p = PyExc::panic("boom", Some("src/a.rs:1:2"), Some("0: frame".to_owned()));
        let b = String::from_utf8(body(
            &info,
            b"An unexpected error has occurred: PanicException: boom",
            Some(&p),
        ))
        .unwrap();
        assert!(b.contains("snp version: 2.0.1\n"));
        assert!(b.contains("panicked at src/a.rs:1:2\nstack backtrace:\n0: frame\n"));
        assert!(b.contains("https://github.com/grok-rs/snp/issues"));
    }
}
