# D-SEC-08

TLS + sha256 verified before extraction; safe extraction

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-16; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-08.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-16 (`crates/snp-net`).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| toolchain downloads (node, go, rustup, ruby helpers) are not verified | TLS + sha256 verified before extraction; safe extraction |

## Rationale

pre-commit 4.6.2 trusts whatever bytes its toolchain bootstraps receive:

* **go** — `golang._install_go` (`golang.py:101-121`) streams `https://dl.google.com/go/go{version}.{os}-{arch}.{ext}`
  with `urllib.request.urlopen` and unpacks it with `tarfile.extractall`/`zipfile.extractall`, no checksum and no
  extraction filter (Python 3.12 still defaults to the `fully_trusted` filter: absolute names, `..`, links out of the
  tree and device files are honoured);
* **node** — `nodeenv --prebuilt` downloads the node archive (`nodeenv.py:811-865`) without checking `SHASUMS256.txt`;
* **rust** — `rust.install_rust_with_toolchain` (`rust.py:83-108`) downloads `https://sh.rustup.rs` and **executes**
  it;
* **ruby** — the vendored `rbenv`/`ruby-build`/`ruby-download` tarballs are unpacked with `tarfile.extractall`
  without a filter (`ruby.py:81-85`).

TLS protects the transport, not the content: a compromised mirror, a corporate TLS-intercepting proxy with a trusted
CA, a poisoned CDN edge or a tampered cache all deliver arbitrary executables that pre-commit then runs as the user,
and a hostile archive can write outside the environment directory. snp (ADR-034, ADR-077, ARCHITECTURE §6.3):

* downloads only over `https` (an `http://` archive mirror needs `SNP_ALLOW_INSECURE_MIRROR=1`, and even then the
  checksum manifest comes over TLS from the canonical origin or an explicit https `SNP_NODE_SHASUMS_URL` /
  `SNP_GO_JSON_URL`);
* verifies the sha256 of every archive **while streaming it into `<rand>.part`** and renames it to its final name only
  after the digest matched — against `SHASUMS256.txt` (node), the go.dev JSON (go), rustup's `.sha256` sidecar
  (rustup-init, a verified binary instead of a piped shell script) or a digest embedded in snp (uv, the ruby helpers);
* extracts in two passes into `<dest>.part-<rand>` (regular files and directories, then symlinks), refusing absolute,
  `..` and drive-prefixed names, device/FIFO/socket entries, hard links that do not point at a regular file extracted
  earlier, and symlinks that leave the tree or chain through another symlink (a `..` in a target must step back over
  a real directory of the tree, which also catches a chain spelled with a case variant on case-insensitive macOS and
  Windows filesystems: `X -> .` + `y -> x/..`); parents are opened without following links (Linux:
  `openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)`); setuid/setgid/sticky bits are stripped; metadata held in memory is
  bounded (1 MiB of tar records between two entries, so a gzip-bombed GNU long name fails as too large; 64 MiB of
  recorded names and link targets); the tree is renamed into place only when complete. Otherwise the result matches
  CPython `tarfile.extractall`: directories without an entry get `0o777 & ~umask`, a hard link's mode applies to the
  shared file.
* resolves node's `default`/`latest`/`lts` to the newest release of `index.json` that is not a pre-release and lists
  a build for the target (the unofficial builds' index for musl and riscv64). nodeenv 1.11.0 takes the first entry
  (only checking the x86_64-musl/riscv64 builds), which in October 2026 is `v27.0.0-alpha.0.0.0` — a pre-release
  everywhere and a 404 on x86_64 macOS (no `darwin-x64` build) — and has no arm64-musl case at all.

The divergence is invisible for genuine downloads: the same archives are installed at the same places. It only
appears when the bytes are not the published ones — then snp fails closed (exit 3, the message names the URL and
both digests) where pre-commit installs and runs them.

## Examples

A go hook whose archive is replaced in transit (scenario `tests/differential/divergences/D-SEC-08.yaml`: a local
HTTPS interception proxy trusted through `SSL_CERT_FILE` serves the genuine go.dev JSON and a tampered
`go1.23.2.linux-amd64.tar.gz` whose `bin/go` prints `pwned`):

pre-commit 4.6.2 installs the tampered toolchain and the hook runs it (exit 1 here, because the fake `go install`
does not produce the hook binary; a real attacker's archive simply succeeds):

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
An unexpected error has occurred: CalledProcessError: command: ('/bin/sh', '<env>/.go/bin/go', 'install', './...')
return code: 1
stdout:
    pwned
stderr: (none)
Check the log at <store>/pre-commit.log
```

snp refuses the archive before anything is extracted (exit 3):

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
An unexpected error has occurred: NetError: checksum mismatch for https://dl.google.com/go/go1.23.2.linux-amd64.tar.gz: expected <go.dev sha256>, got <sha256 of the tampered archive>
Check the log at <store>/snp.log
```

Hermetic coverage of the snp side lives in `crates/snp-net/tests/net`: a tampered `http://` mirror archive is
rejected while the manifest still comes over TLS (`fetch::http_mirror_serves_only_bytes_the_tls_manifest_verifies`),
a checksum mismatch leaves no file at the final name, and the hostile-archive matrix (absolute, `..`, drive letters,
escaping and chained symlinks, hard links out of the tree, devices, FIFOs, setuid) is refused
(`archive::*`; fuzzed by `fuzz/fuzz_targets/net_archive_extract.rs`).
