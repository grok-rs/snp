//! Local test servers for the snp-net integration tests: an HTTPS server (rustls, certificate signed by the fixture
//! CA in `tests/fixtures/tls`, trusted through `SSL_CERT_FILE`), a plain HTTP server (insecure mirrors) and an HTTP
//! CONNECT proxy. Every server records the requests it saw.
//!
//! Owner: WP-16.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use snp_core::env::Env;

/// The fixture directory.
pub(crate) fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The CA that signed the server certificate.
pub(crate) fn ca_file() -> PathBuf {
    fixtures().join("tls/ca.pem")
}

/// An env trusting only the fixture CA, plus `extra`.
pub(crate) fn env_with_ca(extra: &[(&str, &str)]) -> Env {
    let ca = ca_file();
    let mut pairs: Vec<(String, String)> =
        vec![("SSL_CERT_FILE".into(), ca.to_string_lossy().into_owned())];
    pairs.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    Env::from_pairs(pairs)
}

/// One canned response.
#[derive(Clone, Debug)]
pub(crate) struct Reply {
    /// Status code.
    pub(crate) status: u16,
    /// Body bytes.
    pub(crate) body: Vec<u8>,
    /// Extra headers.
    pub(crate) headers: Vec<(String, String)>,
    /// Announce this `Content-Length` instead of the body's length.
    pub(crate) content_length: Option<u64>,
    /// Send the body in small pieces with this pause between them.
    pub(crate) trickle: Option<Duration>,
}

impl Reply {
    /// 200 with `body`.
    pub(crate) fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::status(200, body)
    }

    /// `status` with `body`.
    pub(crate) fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: Vec::new(),
            content_length: None,
            trickle: None,
        }
    }
}

/// Maps a request path to a reply.
pub(crate) type Handler = Arc<dyn Fn(&str) -> Reply + Send + Sync>;

/// A running server; stopped on drop.
pub(crate) struct Server {
    /// The bound address.
    pub(crate) addr: SocketAddr,
    /// `https` or `http`.
    scheme: &'static str,
    /// Requests seen (`GET /path`).
    log: Arc<Mutex<Vec<String>>>,
    /// TCP connections accepted.
    accepted: Arc<AtomicUsize>,
    /// Stop flag.
    stop: Arc<AtomicBool>,
    /// The accept thread.
    accept: Option<JoinHandle<()>>,
}

impl Server {
    /// An HTTPS server whose certificate is signed by the fixture CA.
    pub(crate) fn https(handler: Handler) -> Self {
        let config = Arc::new(tls_config());
        Self::start("https", move |stream, log| {
            let conn = rustls::ServerConnection::new(Arc::clone(&config)).expect("tls conn");
            let mut tls = rustls::StreamOwned::new(conn, stream);
            serve(&mut tls, &handler, log);
            tls.conn.send_close_notify();
            let _ignored = tls.flush();
            let _ignored = tls.sock.shutdown(Shutdown::Both);
        })
    }

    /// A plain HTTP server.
    pub(crate) fn http(handler: Handler) -> Self {
        Self::start("http", move |mut stream, log| {
            serve(&mut stream, &handler, log);
            let _ignored = stream.flush();
            let _ignored = stream.shutdown(Shutdown::Both);
        })
    }

    /// An HTTP CONNECT proxy (logs `CONNECT host:port`).
    pub(crate) fn connect_proxy() -> Self {
        Self::start("http", proxy)
    }

    fn start(
        scheme: &'static str,
        conn: impl Fn(TcpStream, &Mutex<Vec<String>>) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicUsize::new(0));
        let conn = Arc::new(conn);
        let (log2, stop2, accepted2) = (Arc::clone(&log), Arc::clone(&stop), Arc::clone(&accepted));
        let accept = std::thread::Builder::new()
            .name(format!("{scheme}-server"))
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    accepted2.fetch_add(1, Ordering::SeqCst);
                    let (conn, log) = (Arc::clone(&conn), Arc::clone(&log2));
                    let _ignored = std::thread::Builder::new().spawn(move || conn(stream, &log));
                }
            })
            .expect("spawn");
        Self {
            addr,
            scheme,
            log,
            accepted,
            stop,
            accept: Some(accept),
        }
    }

    /// `scheme://127.0.0.1:port` + `path`.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}://{}{}", self.scheme, self.addr, path)
    }

    /// `http://127.0.0.1:port` (proxy URL).
    pub(crate) fn proxy_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// TCP connections accepted so far.
    pub(crate) fn connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// The requests seen so far.
    pub(crate) fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ignored = TcpStream::connect(self.addr);
        if let Some(h) = self.accept.take() {
            let _ignored = h.join();
        }
    }
}

/// The rustls server configuration (fixture certificate and key, ring provider).
fn tls_config() -> rustls::ServerConfig {
    let dir = fixtures().join("tls");
    let cert_pem = std::fs::read(dir.join("server.pem")).unwrap();
    let key_pem = std::fs::read(dir.join("server-key.pem")).unwrap();
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> = ureq::tls::parse_pem(&cert_pem)
        .filter_map(|i| match i.unwrap() {
            ureq::tls::PemItem::Certificate(c) => {
                Some(rustls::pki_types::CertificateDer::from(c.der().to_vec()))
            }
            _ => None,
        })
        .collect();
    let key = ureq::tls::parse_pem(&key_pem)
        .find_map(|i| match i.unwrap() {
            ureq::tls::PemItem::PrivateKey(k) => Some(k.der().to_vec()),
            _ => None,
        })
        .unwrap();
    let key =
        rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(key));
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap()
}

/// Reads one request head; returns the request line.
fn read_head(r: &mut dyn Read) -> Option<String> {
    let mut reader = BufReader::new(r);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    Some(first.trim_end().to_owned())
}

/// Serves one request on `s`.
fn serve<S: Read + Write>(s: &mut S, handler: &Handler, log: &Mutex<Vec<String>>) {
    let Some(line) = read_head(s) else { return };
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("").to_owned();
    log.lock().unwrap().push(format!("{method} {path}"));
    let reply = handler(&path);
    let len = reply
        .content_length
        .unwrap_or_else(|| u64::try_from(reply.body.len()).unwrap());
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {len}\r\nConnection: close\r\n",
        reply.status
    );
    for (k, v) in &reply.headers {
        let _ignored = write!(head, "{k}: {v}\r\n");
    }
    head.push_str("\r\n");
    if s.write_all(head.as_bytes()).is_err() {
        return;
    }
    match reply.trickle {
        None => {
            let _ignored = s.write_all(&reply.body);
        }
        Some(pause) => {
            let mut rest: &[u8] = &reply.body;
            while !rest.is_empty() {
                let (chunk, tail) = rest.split_at_checked(1024).unwrap_or((rest, &[]));
                if s.write_all(chunk).and_then(|()| s.flush()).is_err() {
                    return;
                }
                rest = tail;
                std::thread::sleep(pause);
            }
        }
    }
    let _ignored = s.flush();
}

/// One CONNECT tunnel.
fn proxy(mut client: TcpStream, log: &Mutex<Vec<String>>) {
    let Some(line) = read_head(&mut client) else {
        return;
    };
    let target = line.split(' ').nth(1).unwrap_or("").to_owned();
    log.lock().unwrap().push(format!("CONNECT {target}"));
    let Ok(upstream) = TcpStream::connect(&target) else {
        let _ignored = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
        return;
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .is_err()
    {
        return;
    }
    let (mut c2, mut u2) = (client.try_clone().unwrap(), upstream.try_clone().unwrap());
    let (mut c1, mut u1) = (client, upstream);
    let t = std::thread::Builder::new()
        .spawn(move || {
            let _ignored = std::io::copy(&mut c2, &mut u2);
            let _ignored = u2.shutdown(Shutdown::Write);
        })
        .unwrap();
    let _ignored = std::io::copy(&mut u1, &mut c1);
    let _ignored = c1.shutdown(Shutdown::Write);
    let _ignored = t.join();
}

/// sha256 hex of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ignored = write!(s, "{b:02x}");
            s
        })
}
