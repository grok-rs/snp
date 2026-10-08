//! `cargo xtask tool-checksums [--uv VERSION] [--manifest FILE] [--check]`: refresh the toolchain catalogue checksums
//! (ARCHITECTURE §6.3, ADR-034).
//!
//! uv is the one downloaded tool whose sha256 is embedded in snp (the trust root is the snp release, not a manifest
//! fetched at run time). This task reads uv's `dist-manifest.json` of the pinned release — downloaded with `curl` over
//! https (`--proto =https --tlsv1.2`), or a local copy given with `--manifest` — and regenerates
//! `crates/snp-net/src/generated/uv_checksums.rs` with one `(target triple, extension, sha256)` row per release
//! archive. Without `--uv` the version currently pinned in that file is refreshed; `--check` compares instead of
//! writing and fails when the committed table differs (a re-tagged release, a hand edit).
//!
//! The other tools need no table: node, go and rustup are verified against manifests fetched over TLS at run time, and
//! the ruby helpers' digests sit next to their embedded bytes (checked by snp-net's unit tests).
//!
//! Owner: WP-16.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// The generated table, relative to the workspace root.
const TABLE: &str = "crates/snp-net/src/generated/uv_checksums.rs";

/// Parsed command line.
#[derive(Debug, Default)]
struct Args {
    /// `--uv VERSION`.
    uv: Option<String>,
    /// `--manifest FILE`.
    manifest: Option<PathBuf>,
    /// `--check`.
    check: bool,
}

/// Parses the task's arguments.
fn parse(args: &[OsString]) -> anyhow::Result<Args> {
    let mut out = Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--check") => out.check = true,
            Some("--uv") => {
                let v = it.next().context("--uv needs a version")?;
                out.uv = Some(v.to_str().context("--uv: not UTF-8")?.to_owned());
            }
            Some("--manifest") => {
                out.manifest = Some(PathBuf::from(it.next().context("--manifest needs a file")?));
            }
            _ => bail!(
                "unknown argument {}; usage: tool-checksums [--uv VERSION] [--manifest FILE] [--check]",
                a.to_string_lossy()
            ),
        }
    }
    Ok(out)
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let args = parse(args)?;
    let root = workspace::root()?;
    let table = root.join(TABLE);
    let version = match &args.uv {
        Some(v) => v.clone(),
        None => pinned_version(&table)?,
    };
    ensure!(
        !version.is_empty()
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b)),
        "invalid uv version {version:?}"
    );
    let url =
        format!("https://github.com/astral-sh/uv/releases/download/{version}/dist-manifest.json");
    let manifest = match &args.manifest {
        Some(p) => {
            std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
        }
        None => curl(&url)?,
    };
    let rows = archives(&manifest)?;
    let rendered = render(&version, &url, &rows);
    if args.check {
        let current =
            std::fs::read_to_string(&table).with_context(|| format!("reading {TABLE}"))?;
        ensure!(
            current == rendered,
            "{TABLE} is stale for uv {version}; run `cargo xtask tool-checksums --uv {version}`"
        );
        return Ok(());
    }
    std::fs::write(&table, rendered).with_context(|| format!("writing {TABLE}"))?;
    workspace::say(&format!("{TABLE}: uv {version}, {} archives", rows.len()));
    Ok(())
}

/// The version pinned in the committed table (`pub(crate) const UV_VERSION: &str = "…";`).
fn pinned_version(table: &Path) -> anyhow::Result<String> {
    let text =
        std::fs::read_to_string(table).with_context(|| format!("reading {}", table.display()))?;
    text.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("pub(crate) const UV_VERSION: &str = \"")
                .and_then(|r| r.strip_suffix("\";"))
                .map(str::to_owned)
        })
        .context("no UV_VERSION in the table; pass --uv VERSION")
}

/// `curl -fsSL` over https only.
fn curl(url: &str) -> anyhow::Result<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "120",
            url,
        ])
        .output()
        .context("running curl")?;
    ensure!(
        out.status.success(),
        "curl {url} failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).context("dist-manifest.json is not UTF-8")
}

/// `(target, extension, sha256)` of every `uv-<target>.tar.gz|zip` archive of the manifest, sorted by target.
fn archives(manifest: &str) -> anyhow::Result<Vec<(String, String, String)>> {
    let v: serde_json::Value =
        serde_json::from_str(manifest).context("parsing dist-manifest.json")?;
    let arts = v
        .get("artifacts")
        .and_then(serde_json::Value::as_object)
        .context("dist-manifest.json has no `artifacts`")?;
    let mut rows = Vec::new();
    for (name, art) in arts {
        let Some(rest) = name.strip_prefix("uv-") else {
            continue;
        };
        let (target, ext) = if let Some(t) = rest.strip_suffix(".tar.gz") {
            (t, "tar.gz")
        } else if let Some(t) = rest.strip_suffix(".zip") {
            (t, "zip")
        } else {
            continue;
        };
        let triples: Vec<&str> = art
            .get("target_triples")
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        ensure!(
            triples == [target],
            "{name}: target_triples {triples:?} do not match the file name"
        );
        let sha = art
            .get("checksums")
            .and_then(|c| c.get("sha256"))
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("{name} has no sha256"))?;
        ensure!(
            sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "{name}: malformed sha256 {sha:?}"
        );
        rows.push((target.to_owned(), ext.to_owned(), sha.to_ascii_lowercase()));
    }
    ensure!(!rows.is_empty(), "dist-manifest.json lists no uv archive");
    rows.sort();
    Ok(rows)
}

/// The generated Rust source.
fn render(version: &str, url: &str, rows: &[(String, String, String)]) -> String {
    let mut s = String::new();
    s.push_str("//! uv release archives and their sha256 (ARCHITECTURE §6.3: embedded per target, the trust root is the snp\n");
    s.push_str("//! release).\n//!\n");
    writeln!(
        s,
        "//! @generated by `cargo xtask tool-checksums --uv {version}` from"
    )
    .unwrap_or_default();
    writeln!(s, "//! <{url}>; do not edit.").unwrap_or_default();
    s.push_str("//!\n//! Owner: WP-16.\n\n");
    s.push_str("/// The pinned uv release.\n");
    writeln!(s, "pub(crate) const UV_VERSION: &str = \"{version}\";\n").unwrap_or_default();
    s.push_str("/// `(target triple, archive extension, sha256)` of every `uv-<target>.<ext>` archive of the release.\n");
    s.push_str("pub(crate) const UV_ARCHIVES: &[(&str, &str, &str)] = &[\n");
    for (t, e, h) in rows {
        // rustfmt's layout of a tuple too long for one line (the table is formatted like hand-written code).
        writeln!(
            s,
            "    (\n        \"{t}\",\n        \"{e}\",\n        \"{h}\",\n    ),"
        )
        .unwrap_or_default();
    }
    s.push_str("];\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_from_manifest() {
        let m = r#"{"artifacts": {
            "uv-x86_64-unknown-linux-gnu.tar.gz": {"kind": "executable-zip", "target_triples": ["x86_64-unknown-linux-gnu"],
                "checksums": {"sha256": "9167D72B3319674B6303C4CBE071854BBA13EBDF3D76B1A7CBDC175471FB66D6"}},
            "uv-x86_64-pc-windows-msvc.zip": {"target_triples": ["x86_64-pc-windows-msvc"],
                "checksums": {"sha256": "75d05de6762778c31ee183398de7dd15093fad0ed90b1f236d8205ea5ec00c90"}},
            "uv-installer.sh": {"kind": "installer"},
            "source.tar.gz": {}
        }}"#;
        let rows = archives(m).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "x86_64-pc-windows-msvc");
        assert_eq!(
            rows[1].2,
            "9167d72b3319674b6303c4cbe071854bba13ebdf3d76b1a7cbdc175471fb66d6"
        );
        let text = render("0.1.0", "https://x/dist-manifest.json", &rows);
        assert!(text.contains("pub(crate) const UV_VERSION: &str = \"0.1.0\";"));
        assert!(text.contains(
            "    (\n        \"x86_64-pc-windows-msvc\",\n        \"zip\",\n        \"75d05de6"
        ));
    }

    #[test]
    fn mismatching_triples_are_refused() {
        let m = r#"{"artifacts": {"uv-a.tar.gz": {"target_triples": ["b"], "checksums": {"sha256": "00"}}}}"#;
        assert!(archives(m).is_err());
    }
}
