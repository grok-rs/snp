//! `HttpFetch` against local servers (WBS WP-16 acceptance: "local HTTPS test server (rustls, self-signed CA via
//! `SSL_CERT_FILE`): verified download succeeds; checksum mismatch errors and leaves no file at the final name; size
//! cap, retries, `HTTPS_PROXY`, `SNP_OFFLINE` and http-mirror refusal tested").

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::env::Env;
use snp_net::tools::{self, Tool};
use snp_net::{Checksum, DOWNLOAD_CAP, Fetch, HttpFetch, NetError};

use crate::support::{Reply, Server, ca_file, env_with_ca, fixtures, sha256_hex};

const PAYLOAD: &[u8] = b"verified bytes\n";

/// No names / no requests.
const NONE: [String; 0] = [];

fn sum(bytes: &[u8]) -> Checksum {
    Checksum::sha256_hex(&sha256_hex(bytes)).unwrap()
}

fn static_server(body: &'static [u8]) -> Server {
    Server::https(Arc::new(move |_p: &str| Reply::ok(body)))
}

/// Every name in `dir`, sorted.
fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn verified_download_succeeds_and_leaves_only_the_final_file() {
    let s = static_server(PAYLOAD);
    let f = HttpFetch::from_env(&env_with_ca(&[]));
    let d = tempfile::tempdir().unwrap();
    let p = f
        .get_verified(
            &s.url("/a/tool.tar.gz"),
            &sum(PAYLOAD),
            d.path(),
            "tool.tar.gz",
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(p, d.path().join("tool.tar.gz"));
    assert_eq!(std::fs::read(&p).unwrap(), PAYLOAD);
    assert_eq!(names(d.path()), ["tool.tar.gz"]);
    assert_eq!(s.requests(), ["GET /a/tool.tar.gz"]);
}

#[test]
fn checksum_mismatch_errors_and_leaves_no_file() {
    let s = static_server(b"tampered\n");
    let f = HttpFetch::from_env(&env_with_ca(&[]));
    let d = tempfile::tempdir().unwrap();
    let e = f
        .get_verified(
            &s.url("/x"),
            &sum(PAYLOAD),
            d.path(),
            "x.tar.gz",
            &CancelToken::new(),
        )
        .unwrap_err();
    let NetError::Checksum {
        url,
        expected,
        actual,
    } = &e
    else {
        panic!("{e:?}");
    };
    assert_eq!(url, &s.url("/x"));
    assert_eq!(expected, &sha256_hex(PAYLOAD));
    assert_eq!(actual, &sha256_hex(b"tampered\n"));
    assert_eq!(
        e.to_string(),
        format!("checksum mismatch for {url}: expected {expected}, got {actual}")
    );
    assert_eq!(names(d.path()), NONE);
    // A mismatch is not retried.
    assert_eq!(s.requests().len(), 1);
}

#[test]
fn get_text_reads_manifests() {
    let s = static_server(b"abc  file\n");
    let f = HttpFetch::from_env(&env_with_ca(&[]));
    assert_eq!(
        f.get_text(&s.url("/SHASUMS256.txt"), 1024).unwrap(),
        "abc  file\n"
    );
}

#[test]
fn unknown_issuer_is_refused_without_the_ca() {
    let s = static_server(PAYLOAD);
    // Trust only an unrelated CA.
    let other = fixtures().join("tls/other-ca.pem");
    let env = Env::from_pairs([("SSL_CERT_FILE", other.to_str().unwrap())]);
    let f = HttpFetch::from_env(&env);
    let e = f.get_text(&s.url("/x"), 1024).unwrap_err();
    assert!(matches!(e, NetError::Transport { .. }), "{e:?}");
    // Certificate failures are not retried.
    assert_eq!(s.requests(), NONE);
    assert_eq!(s.connections(), 1);
}

#[test]
fn ssl_cert_dir_is_honoured() {
    let s = static_server(PAYLOAD);
    let d = tempfile::tempdir().unwrap();
    std::fs::copy(ca_file(), d.path().join("ca.pem")).unwrap();
    std::fs::write(d.path().join("junk.txt"), b"not a certificate").unwrap();
    let env = Env::from_pairs([("SSL_CERT_DIR", d.path().to_str().unwrap())]);
    let f = HttpFetch::from_env(&env);
    assert_eq!(f.get_text(&s.url("/x"), 1024).unwrap().as_bytes(), PAYLOAD);
}

#[test]
fn empty_trust_store_is_a_clear_error() {
    let d = tempfile::tempdir().unwrap();
    let empty = d.path().join("empty.pem");
    std::fs::write(&empty, b"").unwrap();
    let env = Env::from_pairs([("SSL_CERT_FILE", empty.to_str().unwrap())]);
    let f = HttpFetch::from_env(&env);
    let e = f.get_text("https://127.0.0.1:9/x", 10).unwrap_err();
    assert!(e.to_string().contains("no certificate"), "{e}");
}

#[test]
fn size_caps() {
    let s = static_server(b"0123456789");
    let f = HttpFetch::from_env(&env_with_ca(&[]));
    assert_eq!(f.get_text(&s.url("/ok"), 10).unwrap(), "0123456789");
    let e = f.get_text(&s.url("/big"), 9).unwrap_err();
    assert!(matches!(e, NetError::TooLarge { limit: 9, .. }), "{e:?}");
    // A download announcing more than the cap is refused before its body is read.
    let huge = Server::https(Arc::new(|_p: &str| {
        let mut r = Reply::ok(Vec::new());
        r.content_length = Some(DOWNLOAD_CAP + 1);
        r
    }));
    let d = tempfile::tempdir().unwrap();
    let e = f
        .get_verified(
            &huge.url("/x"),
            &sum(b""),
            d.path(),
            "x",
            &CancelToken::new(),
        )
        .unwrap_err();
    assert!(
        matches!(
            e,
            NetError::TooLarge {
                limit: DOWNLOAD_CAP,
                ..
            }
        ),
        "{e:?}"
    );
    assert_eq!(names(d.path()), NONE);
}

#[test]
fn server_errors_are_retried_three_times() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let s = Server::https(Arc::new(move |_p: &str| {
        if counter.fetch_add(1, Ordering::SeqCst) < 2 {
            Reply::status(503, b"busy".to_vec())
        } else {
            Reply::ok(PAYLOAD)
        }
    }));
    let f = HttpFetch::from_env(&env_with_ca(&[]));
    let d = tempfile::tempdir().unwrap();
    f.get_verified(
        &s.url("/r"),
        &sum(PAYLOAD),
        d.path(),
        "r",
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(s.requests().len(), 3);
    assert_eq!(names(d.path()), ["r"]);

    let always = Server::https(Arc::new(|_p: &str| Reply::status(502, b"down".to_vec())));
    let e = f.get_text(&always.url("/t"), 100).unwrap_err();
    assert!(matches!(e, NetError::Http { status: 502, .. }), "{e:?}");
    assert_eq!(always.requests().len(), 4, "1 attempt + 3 retries");

    let missing = Server::https(Arc::new(|_p: &str| Reply::status(404, b"no".to_vec())));
    let e = f.get_text(&missing.url("/n"), 100).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("GET {}: HTTP 404", missing.url("/n"))
    );
    assert_eq!(missing.requests().len(), 1, "4xx is final");
}

#[test]
fn https_proxy_is_used_and_no_proxy_bypasses_it() {
    let s = static_server(PAYLOAD);
    let proxy = Server::connect_proxy();
    let f = HttpFetch::from_env(&env_with_ca(&[("HTTPS_PROXY", &proxy.proxy_url())]));
    assert_eq!(
        f.get_text(&s.url("/via-proxy"), 1024).unwrap().as_bytes(),
        PAYLOAD
    );
    assert_eq!(proxy.requests(), [format!("CONNECT {}", s.addr)]);
    assert_eq!(s.requests(), ["GET /via-proxy"]);

    let direct = HttpFetch::from_env(&env_with_ca(&[
        ("HTTPS_PROXY", &proxy.proxy_url()),
        ("NO_PROXY", "localhost,127.0.0.1"),
    ]));
    direct.get_text(&s.url("/direct"), 1024).unwrap();
    assert_eq!(proxy.requests().len(), 1, "NO_PROXY bypasses the proxy");
}

#[test]
fn offline_never_touches_the_network() {
    let s = static_server(PAYLOAD);
    let f = HttpFetch::from_env(&env_with_ca(&[("SNP_OFFLINE", "1")]));
    assert!(matches!(
        f.get_text(&s.url("/x"), 10),
        Err(NetError::Offline)
    ));
    let d = tempfile::tempdir().unwrap();
    let e = f
        .get_verified(
            &s.url("/x"),
            &sum(PAYLOAD),
            d.path(),
            "x",
            &CancelToken::new(),
        )
        .unwrap_err();
    assert!(matches!(e, NetError::Offline), "{e:?}");
    assert_eq!(e.to_string(), "network access disabled (SNP_OFFLINE)");
    assert_eq!(s.requests(), NONE);
    assert_eq!(names(d.path()), NONE);
    // SNP_OFFLINE=0 is not offline.
    let on = HttpFetch::from_env(&env_with_ca(&[("SNP_OFFLINE", "0")]));
    on.get_text(&s.url("/y"), 100).unwrap();
}

#[test]
fn http_is_refused_unless_an_insecure_mirror_is_allowed() {
    let plain = Server::http(Arc::new(|_p: &str| Reply::ok(PAYLOAD)));
    let d = tempfile::tempdir().unwrap();
    let strict = HttpFetch::from_env(&env_with_ca(&[]));
    let e = strict
        .get_verified(
            &plain.url("/a"),
            &sum(PAYLOAD),
            d.path(),
            "a",
            &CancelToken::new(),
        )
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("insecure URL refused: {}", plain.url("/a"))
    );
    assert_eq!(plain.requests(), NONE);

    let lax = HttpFetch::from_env(&env_with_ca(&[("SNP_ALLOW_INSECURE_MIRROR", "1")]));
    lax.get_verified(
        &plain.url("/a"),
        &sum(PAYLOAD),
        d.path(),
        "a",
        &CancelToken::new(),
    )
    .unwrap();
    // Manifests never come over plain HTTP, even with the switch.
    let e = lax
        .get_text(&plain.url("/SHASUMS256.txt"), 100)
        .unwrap_err();
    assert!(matches!(e, NetError::Insecure(_)), "{e:?}");
    assert_eq!(plain.requests(), ["GET /a"]);
}

#[test]
fn a_redirect_to_http_is_refused() {
    let plain = Server::http(Arc::new(|_p: &str| Reply::ok(PAYLOAD)));
    let target = plain.url("/plain");
    let s = Server::https(Arc::new(move |_p: &str| {
        let mut r = Reply::status(302, Vec::new());
        r.headers.push(("Location".into(), target.clone()));
        r
    }));
    let f = HttpFetch::from_env(&env_with_ca(&[("SNP_ALLOW_INSECURE_MIRROR", "1")]));
    let e = f.get_text(&s.url("/r"), 100).unwrap_err();
    assert!(matches!(e, NetError::Insecure(_)), "{e:?}");
    assert_eq!(plain.requests(), NONE);
}

#[test]
fn cancellation_stops_a_download_and_removes_the_part_file() {
    let body: Vec<u8> = vec![b'x'; 256 * 1024];
    let digest = sum(&body);
    let s = Server::https(Arc::new(move |_p: &str| {
        let mut reply = Reply::ok(body.clone());
        reply.trickle = Some(Duration::from_millis(20));
        reply
    }));
    let fetch = HttpFetch::from_env(&env_with_ca(&[]));
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let canceller = cancel.clone();
    let thread = std::thread::Builder::new()
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.cancel(CancelReason::Interrupt);
        })
        .unwrap();
    let start = Instant::now();
    let err = fetch
        .get_verified(&s.url("/slow"), &digest, dir.path(), "slow", &cancel)
        .unwrap_err();
    thread.join().unwrap();
    assert!(matches!(err, NetError::Cancelled), "{err:?}");
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(names(dir.path()), NONE);

    // An already cancelled token never starts.
    let err = fetch
        .get_verified(&s.url("/slow"), &digest, dir.path(), "slow", &cancel)
        .unwrap_err();
    assert!(matches!(err, NetError::Cancelled));
}

#[test]
fn file_names_are_validated() {
    let f = HttpFetch::from_env(&env_with_ca(&[]));
    let d = tempfile::tempdir().unwrap();
    for bad in ["../x", "a/b", "", ".."] {
        let e = f
            .get_verified(
                "https://127.0.0.1:9/x",
                &sum(b""),
                d.path(),
                bad,
                &CancelToken::new(),
            )
            .unwrap_err();
        assert!(matches!(e, NetError::Io(_)), "{bad:?}: {e:?}");
    }
}

#[test]
fn embedded_archives_need_no_network() {
    let fetch = HttpFetch::from_env(&Env::from_pairs([("SNP_OFFLINE", "1")]));
    let dir = tempfile::tempdir().unwrap();
    for tool in [Tool::Rbenv, Tool::RubyBuild, Tool::RubyDownload] {
        let art = tools::resolve(tool, "default", tools::host_target(), &fetch).unwrap();
        let path = fetch
            .get_verified(
                &art.url,
                &art.checksum,
                dir.path(),
                "a.tar.gz",
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            tools::embedded(&art.url).unwrap()
        );
        // A wrong digest is still refused.
        let err = fetch
            .get_verified(
                &art.url,
                &sum(b"x"),
                dir.path(),
                "b.tar.gz",
                &CancelToken::new(),
            )
            .unwrap_err();
        assert!(matches!(err, NetError::Checksum { .. }), "{err:?}");
    }
    assert_eq!(names(dir.path()), ["a.tar.gz"]);
}

/// The node flow with an `http://` mirror: the checksum manifest still comes over TLS (`SNP_NODE_SHASUMS_URL`), the
/// mirror serves only the archive, and a tampered mirror archive is rejected (ADR-077).
#[test]
fn http_mirror_serves_only_bytes_the_tls_manifest_verifies() {
    let genuine = b"genuine node archive".to_vec();
    let file = format!(
        "node-v22.11.0-linux-x64.{}",
        if cfg!(feature = "xz") {
            "tar.xz"
        } else {
            "tar.gz"
        }
    );
    let sums = format!("{}  {file}\n", sha256_hex(&genuine));
    let tls = Server::https(Arc::new(move |p: &str| {
        if p == "/dist/v22.11.0/SHASUMS256.txt" {
            Reply::ok(sums.clone().into_bytes())
        } else {
            Reply::status(404, Vec::new())
        }
    }));
    let tampered = Server::http(Arc::new(|_p: &str| {
        Reply::ok(b"evil node archive".to_vec())
    }));
    let honest = Server::http(Arc::new(move |_p: &str| Reply::ok(genuine.clone())));
    let env_for = |mirror: &Server| {
        env_with_ca(&[
            ("SNP_ALLOW_INSECURE_MIRROR", "1"),
            ("SNP_NODE_MIRROR", &mirror.url("/dist")),
            ("SNP_NODE_SHASUMS_URL", &tls.url("/dist")),
        ])
    };
    let d = tempfile::tempdir().unwrap();

    let f = HttpFetch::from_env(&env_for(&tampered));
    let a = tools::resolve(Tool::Node, "22.11.0", "x86_64-unknown-linux-gnu", &f).unwrap();
    assert_eq!(a.url, format!("https://nodejs.org/dist/v22.11.0/{file}"));
    let e = f
        .get_verified(&a.url, &a.checksum, d.path(), &file, &CancelToken::new())
        .unwrap_err();
    assert!(matches!(e, NetError::Checksum { .. }), "{e:?}");
    assert_eq!(names(d.path()), NONE);
    assert_eq!(tls.requests(), ["GET /dist/v22.11.0/SHASUMS256.txt"]);
    assert_eq!(tampered.requests(), [format!("GET /dist/v22.11.0/{file}")]);

    let f = HttpFetch::from_env(&env_for(&honest));
    let a = tools::resolve(Tool::Node, "v22.11.0", "x86_64-unknown-linux-gnu", &f).unwrap();
    f.get_verified(&a.url, &a.checksum, d.path(), &file, &CancelToken::new())
        .unwrap();
    assert_eq!(names(d.path()), [file]);

    // An http:// manifest override is refused outright.
    let f = HttpFetch::from_env(&env_with_ca(&[
        ("SNP_ALLOW_INSECURE_MIRROR", "1"),
        ("SNP_NODE_SHASUMS_URL", &honest.url("/dist")),
    ]));
    let e = tools::resolve(Tool::Node, "22.11.0", "x86_64-unknown-linux-gnu", &f).unwrap_err();
    assert!(matches!(e, NetError::Insecure(_)), "{e:?}");
}
