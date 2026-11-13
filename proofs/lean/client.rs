//! Lean oracle client (ADR-095, docs/VERIFICATION.md §4.1, §4.3): std only, never packaged.
//!
//! A home-crate oracle test (`crates/<crate>/tests/lean_oracle.rs`) includes this file as a `#[cfg(test)]` module
//! at its crate root, so the test-only clippy relaxations (`unwrap`, `panic`, indexing, printing) apply and no
//! dependency is added (snp-pycompat keeps zero dev-dependencies, ADR-056). `#[path]` is relative to the test
//! file's directory only at the crate root; inside an inline module it is relative to `tests/<module>/`.
//!
//! ```ignore
//! #[cfg(test)]
//! #[path = "../../../proofs/lean/client.rs"]
//! mod client;
//!
//! #[cfg(test)]
//! mod lean_oracle {
//!     use crate::client::{Oracle, hex, words};
//!
//!     #[test]
//!     fn split_matches_the_model() {
//!         let Some(mut model) = Oracle::from_env("shlex-model") else {
//!             return;
//!         };
//!         words(&[' ', '\'', 'a'], 7, |w| {
//!             let s: String = w.iter().collect();
//!             let reply = model.ask_op("split", s.as_bytes()).unwrap();
//!             assert_eq!(reply, rust_answer(&s), "input {s:?}");
//!         });
//!     }
//! }
//! ```
//!
//! **Environment.** `SNP_LEAN_ORACLES` names the directory holding the oracle executables (`lake build` puts them
//! in `proofs/lean/.lake/build/bin`; `cargo xtask proofs oracles` sets it). Unset, every oracle test is skipped
//! with a note on stderr, so contributors without Lean see no failure. With `SNP_REQUIRE_LEAN_ORACLES=1` (CI) a
//! skip is a failure.
//!
//! **Protocol.** The oracle runs as `<dir>/<name> --lines` with a cleared environment and the test's stderr. Each
//! request is one line (`<op> <hex>` or just `<hex>`, depending on the oracle); each reply is one line, by
//! convention `OK …` or `ERR <text>` with byte strings hex-encoded. Both sides flush after every line, so one child
//! serves a whole test (about 25 µs per round trip).
//!
//! **Inputs.** [`words`] enumerates a bounded-exhaustive domain; [`SplitMix64`] draws seeded random inputs (the
//! generator of snp-pycompat's `tests/properties.rs`), so a failure is reproducible from its seed.
//!
//! Every item is exercised by the self-tests at the end of this file, so an including test that uses only part of
//! the API sees no `dead_code` warning.

use std::ffi::OsString;
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// The environment variable naming the oracle directory.
pub(crate) const ORACLES_VAR: &str = "SNP_LEAN_ORACLES";
/// The environment variable that turns a skip into a failure when set to `1`.
pub(crate) const REQUIRE_VAR: &str = "SNP_REQUIRE_LEAN_ORACLES";

/// A test switch read from the process environment.
fn switch(name: &str) -> Option<OsString> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test switch selecting the Lean oracles (ADR-095 §4.3)"
    )]
    let value = std::env::var_os(name);
    value.filter(|v| !v.is_empty())
}

/// One running oracle executable speaking the line protocol.
#[derive(Debug)]
pub(crate) struct Oracle {
    name: String,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Oracle {
    /// The oracle `name` from `$SNP_LEAN_ORACLES`, or `None` when the variable is unset (the caller returns early:
    /// the test is skipped, with a note on stderr).
    ///
    /// # Panics
    ///
    /// When the variable is unset and `SNP_REQUIRE_LEAN_ORACLES=1`, or when it is set but the oracle cannot be
    /// started (a misconfiguration is never a silent skip).
    pub(crate) fn from_env(name: &str) -> Option<Self> {
        Self::from_dir(switch(ORACLES_VAR).map(PathBuf::from), name, require())
    }

    /// [`Oracle::from_env`] with the directory and the require switch passed in.
    fn from_dir(dir: Option<PathBuf>, name: &str, required: bool) -> Option<Self> {
        let Some(dir) = dir else {
            assert!(
                !required,
                "{REQUIRE_VAR}=1 but {ORACLES_VAR} is unset: build the oracles (`lake build` in proofs/lean) and \
                 point {ORACLES_VAR} at proofs/lean/.lake/build/bin"
            );
            eprintln!("skipped: {ORACLES_VAR} is unset, so the Lean oracle `{name}` is not run");
            return None;
        };
        let path = dir.join(executable(name));
        match Self::spawn(&path) {
            Ok(oracle) => Some(oracle),
            Err(e) => panic!("cannot start the Lean oracle {}: {e}", path.display()),
        }
    }

    /// Starts `path --lines` with a cleared environment, piped stdin/stdout and the test's stderr.
    pub(crate) fn spawn(path: &Path) -> io::Result<Self> {
        let mut child = Command::new(path)
            .arg("--lines")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let not_piped = || io::Error::other("oracle stdio was not piped");
        let stdin = child.stdin.take().ok_or_else(not_piped)?;
        let stdout = child.stdout.take().ok_or_else(not_piped)?;
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        Ok(Self {
            name,
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    /// The executable's file name, for assertion messages.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Sends one request line (without its newline) and returns the reply line without its line ending.
    ///
    /// A request containing a line break, or an oracle that exits or closes stdout, is an error.
    pub(crate) fn ask(&mut self, request: &str) -> io::Result<String> {
        if request.contains(['\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a request is one line: hex-encode the payload",
            ));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        stdin.write_all(request.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        let mut reply = String::new();
        if self.stdout.read_line(&mut reply)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("oracle {} closed its output", self.name),
            ));
        }
        Ok(reply.trim_end_matches(['\n', '\r']).to_owned())
    }

    /// `ask("<op> <hex(input)>")`.
    pub(crate) fn ask_op(&mut self, op: &str, input: &[u8]) -> io::Result<String> {
        self.ask(&format!("{op} {}", hex(input)))
    }

    /// `ask("<hex(input)>")`, for single-operation oracles.
    pub(crate) fn ask_hex(&mut self, input: &[u8]) -> io::Result<String> {
        self.ask(&hex(input))
    }
}

impl Drop for Oracle {
    /// Closes the oracle's stdin (its read loop ends on EOF) and reaps it.
    fn drop(&mut self) {
        drop(self.stdin.take());
        if self.child.wait().is_err() {
            self.child.kill().unwrap_or_default();
        }
    }
}

/// Whether `SNP_REQUIRE_LEAN_ORACLES=1` is set.
pub(crate) fn require() -> bool {
    is_one(switch(REQUIRE_VAR))
}

/// A switch value of exactly `1`.
fn is_one(value: Option<OsString>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// `name` with the platform's executable suffix (`.exe` on Windows).
fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Lowercase hex of `bytes` (the protocol's byte-string encoding).
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::new();
    for &b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

/// Decodes hex (either case); `None` for an odd length or a non-hex digit.
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let raw = s.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        out.push(nibble(raw[i])? << 4 | nibble(raw[i + 1])?);
        i += 2;
    }
    Some(out)
}

/// Calls `f` on every word over `alphabet` of length `0..=max_len`, shortest first (`Σ |alphabet|^k` calls).
pub(crate) fn words<T: Copy>(alphabet: &[T], max_len: usize, mut f: impl FnMut(&[T])) {
    let mut word: Vec<T> = Vec::new();
    let mut digits: Vec<usize> = Vec::new();
    for len in 0..=max_len {
        if len > 0 && alphabet.is_empty() {
            return;
        }
        digits.clear();
        digits.resize(len, 0);
        loop {
            word.clear();
            word.extend(digits.iter().map(|&d| alphabet[d]));
            f(&word);
            // Odometer increment; done when every position wrapped.
            let mut pos = 0;
            while pos < len {
                digits[pos] += 1;
                if digits[pos] < alphabet.len() {
                    break;
                }
                digits[pos] = 0;
                pos += 1;
            }
            if pos == len {
                break;
            }
        }
    }
}

/// `SplitMix64`: a tiny deterministic generator for seeded random inputs (print the seed in failure messages).
#[derive(Debug, Clone)]
pub(crate) struct SplitMix64(pub(crate) u64);

impl SplitMix64 {
    /// The next 64 random bits.
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform-ish in `0..n` (`n > 0`).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX).max(1);
        usize::try_from(self.next_u64() % n64).unwrap_or(0)
    }

    /// A word of length `0..=max_len` over `alphabet` (non-empty).
    pub(crate) fn word<T: Copy>(&mut self, alphabet: &[T], max_len: usize) -> Vec<T> {
        let len = self.below(max_len + 1);
        (0..len)
            .map(|_| alphabet[self.below(alphabet.len())])
            .collect()
    }
}

/// Self-tests of the client: they run in every crate that includes it, need no oracle, and keep every item used.
#[test]
fn lean_client_self_test() {
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(hex(b"\x00\x7f\xff"), "007fff");
    assert_eq!(unhex(&hex(&all)), Some(all));
    assert_eq!(unhex("0A"), Some(vec![10]));
    assert_eq!(unhex("0"), None);
    assert_eq!(unhex("zz"), None);

    let mut seen = Vec::new();
    words(&['a', 'b'], 2, |w| seen.push(w.iter().collect::<String>()));
    assert_eq!(seen, ["", "a", "b", "aa", "ba", "ab", "bb"]);
    let mut count = 0;
    words::<u8>(&[], 3, |_| count += 1);
    assert_eq!(count, 1);

    let (mut a, mut b) = (SplitMix64(7), SplitMix64(7));
    assert_eq!(a.next_u64(), b.next_u64());
    assert!(a.below(10) < 10);
    assert!(a.word(b"xy", 4).len() <= 4);

    // Unset directory: a skip, or a failure when required.
    assert!(Oracle::from_dir(None, "none", false).is_none());
    assert!(std::panic::catch_unwind(|| Oracle::from_dir(None, "none", true)).is_err());
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-lean-oracle");
    assert!(Oracle::spawn(&missing).is_err());
    assert_eq!(executable("m").trim_end_matches(".exe"), "m");
    assert!(ORACLES_VAR.starts_with("SNP_") && REQUIRE_VAR.starts_with("SNP_"));
    assert!(is_one(Some("1".into())) && !is_one(Some("0".into())) && !is_one(None));
    assert_eq!(require(), is_one(switch(REQUIRE_VAR)));
    if let Ok(mut o) = Oracle::spawn(&missing) {
        // Never reached (the executable does not exist); keeps the request API used in every includer.
        assert!(o.ask("a\nb").is_err());
        assert!(o.ask_op("op", b"").is_ok() && o.ask_hex(b"").is_ok() && !o.name().is_empty());
    }
}
