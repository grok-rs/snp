//! The `Fetch` seam (S12) and its ureq implementation (ARCHITECTURE §1.4.9, §6.3; ADR-034, ADR-077).
//!
//! pre-commit 4.6.2 downloads go archives with `urllib.request.urlopen` and leaves node, rustup and ruby to nodeenv,
//! `curl … | sh` and its vendored tarballs, verifying nothing beyond TLS (`golang.py:95-112`, D-SEC-08). snp's
//! downloads are TLS-only (an `http://` archive mirror needs `SNP_ALLOW_INSECURE_MIRROR`; checksum manifests are
//! never fetched over plain HTTP), sha256-verified while streaming, and renamed to their final name only after the
//! digest matched, so a mismatching or interrupted download never leaves a file under that name.
//!
//! Owner: WP-16.

use std::fmt::Debug;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use sha2::{Digest, Sha256};
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use ureq::Agent;

use crate::error::to_hex;
use crate::http::{Agents, IDLE_TIMEOUT};
use crate::policy::{Policy, display, scheme};
use crate::util::{check_file_name, random_suffix};
use crate::{Checksum, NetError};

/// Seam for tests (S12).
pub trait Fetch: Send + Sync + Debug {
    /// A small text resource (checksum manifests, version JSON) of at most `max` bytes.
    fn get_text(&self, url: &str, max: u64) -> Result<String, NetError>;
    /// Streams to `dest_dir/<rand>.part`, hashing on the fly; renames to `file_name` only after the checksum matches.
    fn get_verified(
        &self,
        url: &str,
        sum: &Checksum,
        dest_dir: &Path,
        file_name: &str,
        cancel: &CancelToken,
    ) -> Result<PathBuf, NetError>;
}

/// Hard cap on one verified download (the largest toolchain archive, go, is ~75 MB).
pub const DOWNLOAD_CAP: u64 = 512 << 20;

/// Delays before the 1st, 2nd and 3rd retry of a request that failed with a 5xx status or an I/O error.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// Copy buffer of the streaming download (cancellation is observed between chunks).
const CHUNK: usize = 64 << 10;

/// `ureq::Agent`: rustls + platform verifier; env proxies; 30 s connect, 60 s idle, 3 retries.
///
/// Built from the captured environment ([`HttpFetch::from_env`]); the agents themselves are created lazily on the
/// first network access, so a run that downloads nothing never loads trust roots.
#[derive(Debug)]
pub struct HttpFetch {
    /// The env-derived policy.
    policy: Policy,
    /// The agents (or why they cannot be built).
    agents: OnceLock<Result<Agents, String>>,
}

impl HttpFetch {
    /// `SSL_CERT_FILE`/`DIR`, `HTTPS_PROXY`/`NO_PROXY`, `SNP_OFFLINE`, `SNP_ALLOW_INSECURE_MIRROR`.
    ///
    /// Also the mirror overrides of ARCHITECTURE §6.3: `SNP_NODE_MIRROR` (node archives),
    /// `SNP_NODE_SHASUMS_URL` and `SNP_GO_JSON_URL` (https checksum manifests). Proxy variables follow Python's
    /// `urllib.request.getproxies_environment` (lowercase first; `HTTP_PROXY` ignored under CGI) plus `ALL_PROXY`.
    pub fn from_env(env: &Env) -> Self {
        Self {
            policy: Policy::from_env(env),
            agents: OnceLock::new(),
        }
    }

    /// The agents, built on first use.
    fn agents(&self, shown: &str) -> Result<&Agents, NetError> {
        self.agents
            .get_or_init(|| Agents::build(&self.policy, IDLE_TIMEOUT))
            .as_ref()
            .map_err(|msg| transport(shown, msg.clone()))
    }

    /// Policy checks shared by every network access: configuration errors, then `SNP_OFFLINE`.
    fn preflight(&self, shown: &str) -> Result<(), NetError> {
        if let Some(msg) = &self.policy.config_error {
            return Err(transport(shown, msg.clone()));
        }
        if self.policy.offline {
            return Err(NetError::Offline);
        }
        Ok(())
    }

    /// The agent for an archive URL: https always, http only with `SNP_ALLOW_INSECURE_MIRROR`.
    fn archive_agent(&self, real: &str, shown: &str) -> Result<&Agent, NetError> {
        match scheme(real).as_deref() {
            Some("https") => {
                self.preflight(shown)?;
                Ok(&self.agents(shown)?.https)
            }
            Some("http") => {
                if !self.policy.allow_insecure {
                    return Err(NetError::Insecure(shown.to_owned()));
                }
                self.preflight(shown)?;
                self.agents(shown)?
                    .http
                    .as_ref()
                    .ok_or_else(|| NetError::Insecure(shown.to_owned()))
            }
            _ => Err(transport(shown, "unsupported URL scheme".to_owned())),
        }
    }
}

/// One failed attempt: retry it (5xx, I/O, timeouts) or give up.
enum Step {
    /// Worth another attempt.
    Retry(NetError),
    /// Final.
    Fail(NetError),
}

impl From<io::Error> for Step {
    fn from(e: io::Error) -> Self {
        Self::Fail(NetError::Io(e))
    }
}

/// A transport error naming `url`.
fn transport(url: &str, msg: String) -> NetError {
    NetError::Transport {
        url: url.to_owned(),
        msg,
    }
}

/// Classifies a ureq error of a request to `shown`.
fn classify(e: ureq::Error, shown: &str) -> Step {
    use ureq::Error as E;
    match e {
        E::Io(io) => Step::Retry(transport(shown, io.to_string())),
        E::Timeout(t) => Step::Retry(transport(shown, format!("timed out ({t})"))),
        E::HostNotFound => Step::Retry(transport(shown, "host not found".to_owned())),
        E::ConnectionFailed => Step::Retry(transport(shown, "connection failed".to_owned())),
        E::BodyStalled => Step::Retry(transport(shown, "body stalled".to_owned())),
        E::Protocol(p) => Step::Retry(transport(shown, p.to_string())),
        E::RequireHttpsOnly(u) => Step::Fail(NetError::Insecure(display(&u))),
        E::BodyExceedsLimit(limit) => Step::Fail(NetError::TooLarge {
            url: shown.to_owned(),
            limit,
        }),
        other => Step::Fail(transport(shown, other.to_string())),
    }
}

/// A body read error (the connection died mid-body): retried.
fn body_error(e: &io::Error, shown: &str) -> Step {
    Step::Retry(transport(shown, e.to_string()))
}

/// Status handling shared by both requests: 2xx passes, 5xx is retried, everything else is final.
fn check_status(status: u16, shown: &str) -> Result<(), Step> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let e = NetError::Http {
        url: shown.to_owned(),
        status,
    };
    Err(if (500..600).contains(&status) {
        Step::Retry(e)
    } else {
        Step::Fail(e)
    })
}

/// Runs `attempt` up to 1 + 3 times; `wait` sleeps between attempts and returns false when interrupted.
fn with_retries<T>(
    mut attempt: impl FnMut() -> Result<T, Step>,
    mut wait: impl FnMut(Duration) -> bool,
) -> Result<T, NetError> {
    let mut delays = RETRY_DELAYS.iter();
    loop {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(Step::Fail(e)) => return Err(e),
            Err(Step::Retry(e)) => match delays.next() {
                Some(d) => {
                    if !wait(*d) {
                        return Err(NetError::Cancelled);
                    }
                }
                None => return Err(e),
            },
        }
    }
}

impl Fetch for HttpFetch {
    fn get_text(&self, url: &str, max: u64) -> Result<String, NetError> {
        let real = self.policy.manifest_url(url);
        let shown = display(&real);
        match scheme(&real).as_deref() {
            Some("https") => {}
            // Checksum manifests and version lists are trust roots: never over plain HTTP (ADR-077).
            Some("http") => return Err(NetError::Insecure(shown)),
            _ => return Err(transport(&shown, "unsupported URL scheme".to_owned())),
        }
        self.preflight(&shown)?;
        let agent = &self.agents(&shown)?.https;
        let bytes = with_retries(
            || text_once(agent, &real, &shown, max),
            |d| {
                std::thread::sleep(d);
                true
            },
        )?;
        String::from_utf8(bytes).map_err(|_| transport(&shown, "response is not UTF-8".to_owned()))
    }

    fn get_verified(
        &self,
        url: &str,
        sum: &Checksum,
        dest_dir: &Path,
        file_name: &str,
        cancel: &CancelToken,
    ) -> Result<PathBuf, NetError> {
        check_file_name(file_name)?;
        if cancel.is_cancelled() {
            return Err(NetError::Cancelled);
        }
        if let Some(bytes) = crate::tools::embedded_bytes(url) {
            // Embedded archives (the ruby helpers): verified and written like a download, offline-safe.
            return write_verified(url, sum, dest_dir, file_name, |part| {
                part.file.write_all(bytes)?;
                part.hasher.update(bytes);
                Ok(())
            });
        }
        let real = self.policy.archive_url(url);
        let shown = display(&real);
        let agent = self.archive_agent(&real, &shown)?;
        write_verified(&shown, sum, dest_dir, file_name, |part| {
            with_retries(
                || download_once(agent, &real, &shown, part, cancel),
                |d| !cancel.wait_timeout(d),
            )
        })
    }
}

/// One GET of a small text resource.
fn text_once(agent: &Agent, real: &str, shown: &str, max: u64) -> Result<Vec<u8>, Step> {
    let resp = agent.get(real).call().map_err(|e| classify(e, shown))?;
    check_status(resp.status().as_u16(), shown)?;
    let too_large = || {
        Step::Fail(NetError::TooLarge {
            url: shown.to_owned(),
            limit: max,
        })
    };
    let body = resp.into_body();
    if body.content_length().is_some_and(|len| len > max) {
        return Err(too_large());
    }
    let mut out = Vec::new();
    body.into_reader()
        .take(max.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| body_error(&e, shown))?;
    if u64::try_from(out.len()).map_or(true, |n| n > max) {
        return Err(too_large());
    }
    Ok(out)
}

/// The `.part` file of a download in progress and the running digest of its bytes.
struct Part {
    /// The open `.part` file.
    file: File,
    /// sha256 of the bytes written so far.
    hasher: Sha256,
}

impl Part {
    /// Restarts the download from byte 0.
    fn reset(&mut self) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.rewind()?;
        self.hasher = Sha256::new();
        Ok(())
    }
}

/// One streaming GET into `part` (restarted from scratch on every attempt).
fn download_once(
    agent: &Agent,
    real: &str,
    shown: &str,
    part: &mut Part,
    cancel: &CancelToken,
) -> Result<(), Step> {
    part.reset()?;
    let resp = agent
        .get(real)
        // Byte-exact transfer: a server must not gzip an archive in transit (the digest is over the file bytes).
        .header("Accept-Encoding", "identity")
        .call()
        .map_err(|e| classify(e, shown))?;
    check_status(resp.status().as_u16(), shown)?;
    let too_large = || {
        Step::Fail(NetError::TooLarge {
            url: shown.to_owned(),
            limit: DOWNLOAD_CAP,
        })
    };
    let body = resp.into_body();
    if body.content_length().is_some_and(|len| len > DOWNLOAD_CAP) {
        return Err(too_large());
    }
    let mut reader = body.into_reader();
    let mut buf = vec![0_u8; CHUNK];
    let mut total: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(Step::Fail(NetError::Cancelled));
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(body_error(&e, shown)),
        };
        let chunk = buf.get(..n).unwrap_or_default();
        total = total.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if total > DOWNLOAD_CAP {
            return Err(too_large());
        }
        part.file.write_all(chunk)?;
        part.hasher.update(chunk);
    }
}

/// Creates `dest_dir/<rand>.part`, lets `fill` write it, verifies the digest and renames it to `file_name`; the
/// `.part` file is removed on every failure.
fn write_verified(
    shown: &str,
    sum: &Checksum,
    dest_dir: &Path,
    file_name: &str,
    fill: impl FnOnce(&mut Part) -> Result<(), NetError>,
) -> Result<PathBuf, NetError> {
    let (part_path, file) = create_part(dest_dir)?;
    let mut part = Part {
        file,
        hasher: Sha256::new(),
    };
    let result = fill(&mut part).and_then(|()| {
        let actual: [u8; 32] = part.hasher.clone().finalize().into();
        let Checksum::Sha256(expected) = sum;
        if actual != *expected {
            return Err(NetError::Checksum {
                url: shown.to_owned(),
                expected: sum.hex(),
                actual: to_hex(&actual),
            });
        }
        part.file.sync_all()?;
        Ok(())
    });
    drop(part);
    let final_path = dest_dir.join(file_name);
    let result =
        result.and_then(|()| rename_into_place(&part_path, &final_path).map_err(NetError::from));
    if let Err(e) = result {
        let _ignored = std::fs::remove_file(&part_path);
        return Err(e);
    }
    Ok(final_path)
}

/// A fresh `dest_dir/<rand>.part` (`O_EXCL`; retried on a name collision).
fn create_part(dest_dir: &Path) -> io::Result<(PathBuf, File)> {
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free .part name");
    for _ in 0..16 {
        let path = dest_dir.join(format!("{}.part", random_suffix()));
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// `rename(from, to)`; Windows retries sharing violations (ADR-068).
pub(crate) fn rename_into_place(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        snp_os::fs::rename_with_retry(from, to)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
}
