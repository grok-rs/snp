//! The ureq agents behind [`crate::HttpFetch`]: rustls with the platform verifier (or the `SSL_CERT_FILE`/`SSL_CERT_DIR`
//! roots), env proxies, 30 s connect, 60 s idle reads (ARCHITECTURE §1.4.9, 07 §3.11).
//!
//! ureq has no per-read idle timeout (`timeout_recv_body` is a total budget), so the transports are wrapped by
//! [`IdleConnector`], which clamps every wait for input or output to the idle bound: a stalled connection fails after
//! 60 s without capping how long a large, steadily progressing download may take.
//!
//! Owner: WP-16.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use snp_core::profile::SNP_VERSION;
use ureq::config::Config;
use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::time::Duration as UreqDuration;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
};
use ureq::{Agent, Proxy};

use crate::policy::Policy;

/// Connection (incl. proxy negotiation and TLS handshake) timeout.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest time without progress while sending or receiving.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Redirects followed per request (go.dev → dl.google.com, GitHub → its CDN).
const MAX_REDIRECTS: u32 = 10;
/// Cap on one trust-root file (`SSL_CERT_FILE` or a file in `SSL_CERT_DIR`).
const MAX_CERT_FILE: u64 = 16 << 20;

/// One agent per URL scheme: `https` (https-only, so a redirect can never downgrade) and, when the policy allows
/// insecure mirrors, `http`.
#[derive(Debug)]
pub(crate) struct Agents {
    /// For `https://` URLs.
    pub(crate) https: Agent,
    /// For `http://` URLs (`SNP_ALLOW_INSECURE_MIRROR`).
    pub(crate) http: Option<Agent>,
}

impl Agents {
    /// Builds the agents; an unusable proxy or trust-root setting is an error text.
    pub(crate) fn build(p: &Policy, idle: Duration) -> Result<Self, String> {
        let tls = tls_config(p)?;
        let https_proxy = p
            .https_proxy
            .as_deref()
            .map(|u| proxy(u, p.no_proxy.as_deref()))
            .transpose()?;
        let https = agent(config(&tls, https_proxy, true), idle);
        let http = if p.allow_insecure {
            let http_proxy = p
                .http_proxy
                .as_deref()
                .map(|u| proxy(u, p.no_proxy.as_deref()))
                .transpose()?;
            Some(agent(config(&tls, http_proxy, false), idle))
        } else {
            None
        };
        Ok(Self { https, http })
    }
}

/// The agent configuration shared by both schemes.
fn config(tls: &TlsConfig, proxy: Option<Proxy>, https_only: bool) -> Config {
    Agent::config_builder()
        .http_status_as_error(false)
        .https_only(https_only)
        .proxy(proxy)
        .tls_config(tls.clone())
        .user_agent(format!("snp/{SNP_VERSION}"))
        .max_redirects(MAX_REDIRECTS)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .build()
}

/// An agent whose transports observe the idle bound.
fn agent(config: Config, idle: Duration) -> Agent {
    Agent::with_parts(
        config,
        IdleConnector {
            inner: DefaultConnector::new(),
            idle,
        },
        DefaultResolver::default(),
    )
}

/// `proxy_url` with the `NO_PROXY` patterns; credentials stay inside the proxy (never in an error text).
fn proxy(proxy_url: &str, no_proxy: Option<&str>) -> Result<Proxy, String> {
    let invalid = |e: ureq::Error| {
        format!(
            "invalid proxy URL {}: {e}",
            crate::error::scrub_url(proxy_url)
        )
    };
    let parsed = Proxy::new(proxy_url).map_err(invalid)?;
    let mut b = Proxy::builder(parsed.protocol())
        .host(parsed.host())
        .port(parsed.port());
    if let Some(user) = parsed.username() {
        b = b.username(user);
    }
    if let Some(pass) = parsed.password() {
        b = b.password(pass);
    }
    for pattern in no_proxy
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        b = b.no_proxy(pattern);
    }
    b.build().map_err(invalid)
}

/// The platform verifier, or exactly the certificates of `SSL_CERT_FILE` / `SSL_CERT_DIR` when either is set
/// (OpenSSL semantics: they replace the default trust store).
fn tls_config(p: &Policy) -> Result<TlsConfig, String> {
    if p.cert_file.is_none() && p.cert_dirs.is_empty() {
        return Ok(TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build());
    }
    let mut certs: Vec<Certificate<'static>> = Vec::new();
    if let Some(file) = &p.cert_file {
        load_pem_certs(file, &mut certs)
            .map_err(|e| format!("SSL_CERT_FILE {}: {e}", file.display()))?;
    }
    for dir in &p.cert_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                // Unreadable or non-PEM files in a certificate directory are skipped, as OpenSSL does.
                let _ignored = load_pem_certs(&path, &mut certs);
            }
        }
    }
    if certs.is_empty() {
        return Err("SSL_CERT_FILE/SSL_CERT_DIR contain no certificate".to_owned());
    }
    Ok(TlsConfig::builder()
        .root_certs(RootCerts::new_with_certs(&certs))
        .build())
}

/// Appends every certificate of the PEM file at `path` to `out`.
fn load_pem_certs(path: &Path, out: &mut Vec<Certificate<'static>>) -> Result<(), String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(MAX_CERT_FILE).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    for item in ureq::tls::parse_pem(&bytes) {
        if let Ok(PemItem::Certificate(c)) = item {
            out.push(c.to_owned());
        }
    }
    Ok(())
}

/// Wraps ureq's default connector chain so every transport observes [`IDLE_TIMEOUT`].
#[derive(Debug)]
struct IdleConnector {
    /// TCP, CONNECT proxy and rustls.
    inner: DefaultConnector,
    /// The idle bound.
    idle: Duration,
}

impl Connector<()> for IdleConnector {
    type Out = IdleTransport;

    fn connect(
        &self,
        details: &ConnectionDetails<'_>,
        chained: Option<()>,
    ) -> Result<Option<IdleTransport>, ureq::Error> {
        Ok(self
            .inner
            .connect(details, chained)?
            .map(|inner| IdleTransport {
                inner,
                idle: self.idle,
            }))
    }
}

/// A transport whose waits are clamped to the idle bound.
#[derive(Debug)]
struct IdleTransport {
    /// The real transport.
    inner: Box<dyn Transport>,
    /// The idle bound.
    idle: Duration,
}

impl IdleTransport {
    /// `timeout` shortened to the idle bound (the reason is kept for the error text).
    ///
    /// `NextTimeout` lives in ureq's `unversioned` module (no semver guarantee; 3.5.0 added fields), so it is
    /// copied and only `after` is replaced, never rebuilt from a struct literal.
    fn clamp(&self, timeout: NextTimeout) -> NextTimeout {
        if timeout.after.is_not_happening() || *timeout.after > self.idle {
            let mut t = timeout;
            t.after = UreqDuration::Exact(self.idle);
            t
        } else {
            timeout
        }
    }
}

impl Transport for IdleTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        let t = self.clamp(timeout);
        self.inner.transmit_output(amount, t)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let t = self.clamp(timeout);
        self.inner.await_input(t)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
