//! The toolchain catalogue (WBS WP-16 acceptance: "the tools catalogue resolves URL + checksum for every release
//! target triple"), against a fake `Fetch` serving canned manifests in the real formats, plus a live check against
//! the real origins (`#[ignore = "net"]`, nightly).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use snp_core::cancel::CancelToken;
use snp_net::tools::{self, Layout, Tool};
use snp_net::{Checksum, Fetch, NetError};

/// The release targets of snp (WP-37: Linux gnu/musl × `x86_64`/arm64, macOS × 2, Windows msvc × 2).
const RELEASE_TARGETS: [&str; 8] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

/// A deterministic fake digest for `name`.
fn digest(name: &str) -> String {
    crate::support::sha256_hex(name.as_bytes())
}

/// Serves canned manifests; records every URL asked for.
#[derive(Debug, Default)]
struct Canned {
    asked: Mutex<Vec<String>>,
}

const NODE: &str = "22.11.0";
const GO: &str = "1.23.2";
const RUSTUP: &str = "1.28.2";

fn node_files(musl: bool) -> Vec<String> {
    let plats: &[&str] = if musl {
        &["linux-x64-musl", "linux-arm64-musl"]
    } else {
        &["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"]
    };
    let mut v: Vec<String> = plats
        .iter()
        .flat_map(|p| {
            [
                format!("node-v{NODE}-{p}.tar.gz"),
                format!("node-v{NODE}-{p}.tar.xz"),
            ]
        })
        .collect();
    if !musl {
        v.push(format!("node-v{NODE}-win-x64.zip"));
        v.push(format!("node-v{NODE}-win-arm64.zip"));
    }
    v
}

fn go_json() -> String {
    let mut files = Vec::new();
    for (os, arch, ext) in [
        ("linux", "amd64", "tar.gz"),
        ("linux", "arm64", "tar.gz"),
        ("darwin", "amd64", "tar.gz"),
        ("darwin", "arm64", "tar.gz"),
        ("windows", "amd64", "zip"),
        ("windows", "arm64", "zip"),
    ] {
        let name = format!("go{GO}.{os}-{arch}.{ext}");
        files.push(format!(
            r#"{{"filename": "{name}", "os": "{os}", "arch": "{arch}", "version": "go{GO}", "sha256": "{}", "size": 1, "kind": "archive"}}"#,
            digest(&name)
        ));
        files.push(format!(
            r#"{{"filename": "{name}.msi", "os": "{os}", "arch": "{arch}", "version": "go{GO}", "sha256": "{}", "size": 1, "kind": "installer"}}"#,
            digest("installer")
        ));
    }
    format!(
        r#"[{{"version": "go1.24rc1", "stable": false, "files": []}}, {{"version": "go{GO}", "stable": true, "files": [{}]}}]"#,
        files.join(",")
    )
}

impl Fetch for Canned {
    fn get_text(&self, url: &str, _max: u64) -> Result<String, NetError> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(url.to_owned());
        }
        let shasums = |files: Vec<String>| {
            files.iter().fold(String::new(), |mut s, f| {
                let _ignored = writeln!(s, "{}  {f}", digest(f));
                s
            })
        };
        let text = match url {
            u if u == format!("https://nodejs.org/dist/v{NODE}/SHASUMS256.txt") => {
                shasums(node_files(false))
            }
            u if u
                == format!(
                    "https://unofficial-builds.nodejs.org/download/release/v{NODE}/SHASUMS256.txt"
                ) =>
            {
                shasums(node_files(true))
            }
            "https://nodejs.org/dist/index.json" => format!(
                r#"[{{"version": "v23.1.0", "lts": false}}, {{"version": "v{NODE}", "lts": "Iron"}}]"#
            ),
            "https://go.dev/dl/?mode=json&include=all" => go_json(),
            "https://go.dev/dl/?mode=json" => {
                format!(r#"[{{"version": "go{GO}", "stable": true}}]"#)
            }
            "https://static.rust-lang.org/rustup/release-stable.toml" => {
                format!("schema-version = '1'\nversion = '{RUSTUP}'\n")
            }
            u if u.starts_with(&format!(
                "https://static.rust-lang.org/rustup/archive/{RUSTUP}/"
            )) && u.ends_with(".sha256") =>
            {
                let u = u.strip_suffix(".sha256").unwrap_or(u);
                let mut parts = u.rsplit('/');
                let file = parts.next().unwrap_or_default();
                let exe = format!("{}/{file}", parts.next().unwrap_or_default());
                format!("{} *./{}\n", digest(&exe), exe)
            }
            _ => {
                return Err(NetError::Http {
                    url: url.to_owned(),
                    status: 404,
                });
            }
        };
        Ok(text)
    }

    fn get_verified(
        &self,
        url: &str,
        _sum: &Checksum,
        _dest_dir: &Path,
        _file_name: &str,
        _cancel: &CancelToken,
    ) -> Result<PathBuf, NetError> {
        Err(NetError::Insecure(format!(
            "resolve never downloads ({url})"
        )))
    }
}

fn hex(c: &Checksum) -> String {
    c.hex()
}

#[test]
fn every_tool_resolves_for_every_release_target() {
    for target in RELEASE_TARGETS {
        let windows = target.contains("windows");
        let f = Canned::default();

        let uv = tools::resolve(Tool::Uv, "default", target, &f).unwrap();
        let ext = if windows { "zip" } else { "tar.gz" };
        assert_eq!(
            uv.url,
            format!(
                "https://github.com/astral-sh/uv/releases/download/{}/uv-{target}.{ext}",
                tools::UV_PINNED
            )
        );
        assert!(matches!(
            uv.layout,
            Layout::Zip { strip: 0 } | Layout::TarGz { strip: 1 }
        ));
        assert_eq!(windows, matches!(uv.layout, Layout::Zip { .. }), "{target}");

        let node = tools::resolve(Tool::Node, NODE, target, &f).unwrap();
        let file = node.url.rsplit('/').next().unwrap().to_owned();
        assert_eq!(hex(&node.checksum), digest(&file), "{target}");
        let musl = target.contains("musl");
        let base = if musl {
            "https://unofficial-builds.nodejs.org/download/release"
        } else {
            "https://nodejs.org/dist"
        };
        assert!(
            node.url
                .starts_with(&format!("{base}/v{NODE}/node-v{NODE}-")),
            "{}",
            node.url
        );
        if windows {
            assert!(
                Path::new(&file).extension().is_some_and(|e| e == "zip")
                    && matches!(node.layout, Layout::Zip { strip: 1 })
            );
        } else if cfg!(feature = "xz") {
            assert!(
                Path::new(&file).extension().is_some_and(|e| e == "xz")
                    && matches!(node.layout, Layout::TarXz { strip: 1 })
            );
        } else {
            assert!(
                Path::new(&file).extension().is_some_and(|e| e == "gz")
                    && matches!(node.layout, Layout::TarGz { strip: 1 })
            );
        }

        let go = tools::resolve(Tool::Go, GO, target, &f).unwrap();
        let file = go.url.rsplit('/').next().unwrap().to_owned();
        assert!(
            go.url.starts_with("https://dl.google.com/go/"),
            "{}",
            go.url
        );
        assert_eq!(hex(&go.checksum), digest(&file), "{target}");
        assert!(matches!(
            go.layout,
            Layout::TarGz { strip: 1 } | Layout::Zip { strip: 1 }
        ));

        let rustup = tools::resolve(Tool::Rustup, RUSTUP, target, &f).unwrap();
        let exe = if windows {
            "rustup-init.exe"
        } else {
            "rustup-init"
        };
        assert_eq!(
            rustup.url,
            format!("https://static.rust-lang.org/rustup/archive/{RUSTUP}/{target}/{exe}")
        );
        assert_eq!(hex(&rustup.checksum), digest(&format!("{target}/{exe}")));
        assert!(matches!(rustup.layout, Layout::Executable));

        for tool in [Tool::Rbenv, Tool::RubyBuild, Tool::RubyDownload] {
            let a = tools::resolve(tool, "default", target, &f).unwrap();
            assert!(tools::embedded(&a.url).is_some());
        }

        // Manifests were asked for over https at their canonical origins only.
        for url in f.asked.lock().unwrap().iter() {
            assert!(url.starts_with("https://"), "{url}");
        }
    }
}

#[test]
fn version_aliases() {
    let f = Canned::default();
    assert_eq!(
        tools::resolve_version(Tool::Uv, "default", &f).unwrap(),
        tools::UV_PINNED
    );
    assert_eq!(
        tools::resolve_version(Tool::Node, "default", &f).unwrap(),
        "23.1.0"
    );
    assert_eq!(tools::resolve_version(Tool::Node, "lts", &f).unwrap(), NODE);
    assert_eq!(
        tools::resolve_version(Tool::Node, "v18.0.0", &f).unwrap(),
        "18.0.0"
    );
    assert_eq!(tools::resolve_version(Tool::Go, "default", &f).unwrap(), GO);
    assert_eq!(
        tools::resolve_version(Tool::Go, "go1.21.0", &f).unwrap(),
        "1.21.0"
    );
    assert_eq!(
        tools::resolve_version(Tool::Rustup, "latest", &f).unwrap(),
        RUSTUP
    );
    assert_eq!(
        tools::resolve_version(Tool::Rbenv, "default", &f).unwrap(),
        "1.3.2"
    );
    assert_eq!(
        tools::resolve_version(Tool::RubyBuild, "", &f).unwrap(),
        "20251023"
    );
    let go = tools::resolve(Tool::Go, "default", "x86_64-unknown-linux-gnu", &f).unwrap();
    assert_eq!(
        go.url,
        format!("https://dl.google.com/go/go{GO}.linux-amd64.tar.gz")
    );
}

#[test]
fn unknown_versions_targets_and_hostile_versions() {
    let f = Canned::default();
    // Not listed by the origin: 404 for the archive URL (backends map it to pre-commit's texts).
    let e = tools::resolve(Tool::Go, "1.0.0", "x86_64-unknown-linux-gnu", &f).unwrap_err();
    assert_eq!(
        e.to_string(),
        "GET https://dl.google.com/go/go1.0.0.linux-amd64.tar.gz: HTTP 404"
    );
    let e = tools::resolve(Tool::Node, "1.2.3", "x86_64-unknown-linux-gnu", &f).unwrap_err();
    assert!(matches!(e, NetError::Http { status: 404, .. }), "{e:?}");
    let e = tools::resolve(Tool::Rustup, "0.0.1", "x86_64-unknown-linux-gnu", &f).unwrap_err();
    assert!(matches!(e, NetError::Http { status: 404, .. }), "{e:?}");
    // No build for the target.
    for (tool, target) in [
        (Tool::Uv, "wasm32-unknown-unknown"),
        (Tool::Node, "x86_64-unknown-freebsd"),
        (Tool::Go, "sparc64-unknown-linux-gnu"),
    ] {
        let e = tools::resolve(tool, "default", target, &f).unwrap_err();
        assert!(
            matches!(&e, NetError::Io(io) if io.kind() == std::io::ErrorKind::Unsupported),
            "{tool:?} {target}: {e:?}"
        );
    }
    // Only the pinned uv and the embedded ruby helpers exist.
    assert!(tools::resolve(Tool::Uv, "0.1.0", "x86_64-unknown-linux-gnu", &f).is_err());
    assert!(tools::resolve(Tool::Rbenv, "1.0.0", "x86_64-unknown-linux-gnu", &f).is_err());
    // Versions never smuggle URL syntax.
    for v in ["1.2/../../x", "1 2", "1?x=y", "1#frag"] {
        let e = tools::resolve(Tool::Go, v, "x86_64-unknown-linux-gnu", &f).unwrap_err();
        assert!(
            matches!(&e, NetError::Io(io) if io.kind() == std::io::ErrorKind::InvalidInput),
            "{v}: {e:?}"
        );
    }
}

#[test]
fn host_target_has_a_uv_build() {
    let f = Canned::default();
    assert!(tools::resolve(Tool::Uv, tools::UV_PINNED, tools::host_target(), &f).is_ok());
}

/// The real origins (nightly; `cargo test -p snp-net -- --ignored net_`): every manifest parses and yields an artifact
/// for every release target, and the host's uv archive downloads, verifies and extracts.
#[test]
#[ignore = "net"]
fn net_catalogue_against_the_real_origins() {
    use snp_net::HttpFetch;
    use snp_net::archive::{self, Extract};
    let env = snp_core::env::Env::from_pairs([("PATH", "/usr/bin:/bin"), ("HOME", "/nonexistent")]);
    let f = HttpFetch::from_env(&env);
    for target in RELEASE_TARGETS {
        for tool in [Tool::Uv, Tool::Node, Tool::Go, Tool::Rustup] {
            let a = tools::resolve(tool, "default", target, &f)
                .unwrap_or_else(|e| panic!("{tool:?} {target}: {e}"));
            assert!(a.url.starts_with("https://"), "{}", a.url);
        }
    }
    let uv = tools::resolve(Tool::Uv, "default", tools::host_target(), &f).unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = f
        .get_verified(
            &uv.url,
            &uv.checksum,
            d.path(),
            "uv.archive",
            &CancelToken::new(),
        )
        .unwrap();
    let o = |strip| Extract {
        strip_components: strip,
        allow_symlinks_inside: false,
        max_total_bytes: 512 << 20,
    };
    match uv.layout {
        Layout::TarGz { strip } => archive::tar_gz(&p, &d.path().join("uv"), &o(strip)).unwrap(),
        Layout::Zip { strip } => archive::zip(&p, &d.path().join("uv"), &o(strip)).unwrap(),
        other => panic!("{other:?}"),
    }
    let exe = if cfg!(windows) { "uv.exe" } else { "uv" };
    assert!(d.path().join("uv").join(exe).is_file());

    // node (`.tar.xz` with the `xz` feature on Unix): an archive with in-tree symlinks (`bin/npm`).
    let node = tools::resolve(Tool::Node, "lts", tools::host_target(), &f).unwrap();
    let p = f
        .get_verified(
            &node.url,
            &node.checksum,
            d.path(),
            "node.archive",
            &CancelToken::new(),
        )
        .unwrap();
    let o = |strip| Extract {
        strip_components: strip,
        allow_symlinks_inside: true,
        max_total_bytes: 512 << 20,
    };
    let dest = d.path().join("node");
    match node.layout {
        Layout::TarGz { strip } => archive::tar_gz(&p, &dest, &o(strip)).unwrap(),
        Layout::TarXz { strip } => archive::tar_xz(&p, &dest, &o(strip)).unwrap(),
        Layout::Zip { strip } => archive::zip(&p, &dest, &o(strip)).unwrap(),
        other => panic!("{other:?}"),
    }
    let node_exe = if cfg!(windows) {
        dest.join("node.exe")
    } else {
        dest.join("bin/node")
    };
    assert!(node_exe.is_file());
    #[cfg(unix)]
    assert!(std::fs::read_link(dest.join("bin/npm")).is_ok());

    // go (~75 MB; the layout pre-commit moves to `<env>/.go`).
    let go = tools::resolve(Tool::Go, "default", tools::host_target(), &f).unwrap();
    let p = f
        .get_verified(
            &go.url,
            &go.checksum,
            d.path(),
            "go.archive",
            &CancelToken::new(),
        )
        .unwrap();
    let dest = d.path().join(".go");
    match go.layout {
        Layout::TarGz { strip } => archive::tar_gz(&p, &dest, &o(strip)).unwrap(),
        Layout::Zip { strip } => archive::zip(&p, &dest, &o(strip)).unwrap(),
        other => panic!("{other:?}"),
    }
    assert!(
        dest.join(if cfg!(windows) {
            "bin/go.exe"
        } else {
            "bin/go"
        })
        .is_file()
    );
}
