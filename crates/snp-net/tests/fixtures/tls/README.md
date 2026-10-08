# TLS test fixtures (snp-net)

Throwaway certificates for the local HTTPS server of `tests/net` (WBS WP-16: "local HTTPS test server (rustls,
self-signed CA via SSL_CERT_FILE)"). **Test-only: the private key is public and protects nothing.**

| File | What |
|---|---|
| `ca.pem` | self-signed CA (`CN=snp-net test CA`, P-256, valid until 2126) — passed to `HttpFetch` as `SSL_CERT_FILE` |
| `server.pem`, `server-key.pem` | the server certificate for `127.0.0.1` and `localhost`, signed by `ca.pem`, and its PKCS#8 key |
| `other-ca.pem` | an unrelated CA: trusting only it must fail the handshake |

Regenerate (OpenSSL 3): `./regenerate.sh <this directory> <scratch directory>`.
