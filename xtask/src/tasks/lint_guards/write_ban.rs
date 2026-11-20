//! The snp-hooks write ban (ADR-132): a native hook writes the work tree only through `snp_hooks::writer::Writer`,
//! whose first use in a gated hook takes the deferred baseline `git diff`. Clippy enforces it through the crate's own
//! `crates/snp-hooks/clippy.toml`; this guard keeps that file in force:
//!
//! - every key of the workspace `clippy.toml` is in it with the same value (clippy reads the nearest file only, so a
//!   drifted copy would silently drop a workspace rule), `disallowed-methods` as a superset;
//! - its `disallowed-methods` and `disallowed-types` hold the whole ban ([`BANNED_METHODS`], [`BANNED_TYPES`]);
//! - in the crate's non-test sources only `src/writer.rs` expects or allows `clippy::disallowed_methods` /
//!   `clippy::disallowed_types` (a `#[cfg(test)]` item or module may: test fixtures write files).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use syn::visit::{self, Visit};

/// The crate whose write ban is guarded.
pub(crate) const CRATE: &str = "crates/snp-hooks";
/// Its only write site.
const WRITER: &str = "src/writer.rs";

/// The write APIs banned outside the writer module (panel-parity §2 plus the other `std::fs` mutators, the
/// mode/time/owner setters that work on a handle opened read-only, and every work-tree write API of `snp_os`).
pub(crate) const BANNED_METHODS: [&str; 40] = [
    "std::fs::write",
    "std::fs::OpenOptions::open",
    "std::fs::File::create",
    "std::fs::File::create_new",
    "std::fs::File::options",
    "std::fs::File::set_len",
    "std::fs::copy",
    "std::fs::rename",
    "std::fs::remove_file",
    "std::fs::remove_dir",
    "std::fs::remove_dir_all",
    "std::fs::create_dir",
    "std::fs::create_dir_all",
    "std::fs::hard_link",
    "std::fs::soft_link",
    "std::fs::set_permissions",
    "std::fs::set_times",
    "std::fs::set_times_nofollow",
    "std::fs::File::set_permissions",
    "std::fs::File::set_times",
    "std::fs::File::set_modified",
    "std::fs::DirBuilder::create",
    "std::os::unix::fs::chown",
    "std::os::unix::fs::fchown",
    "std::os::unix::fs::lchown",
    "std::os::unix::fs::symlink",
    "std::os::windows::fs::symlink_file",
    "std::os::windows::fs::symlink_dir",
    "snp_os::fs::rewrite_in_place",
    "snp_os::fs::atomic_replace",
    "snp_os::fs::write_new_durable",
    "snp_os::fs::copy_into_new_durable",
    "snp_os::fs::create_truncate_nofollow",
    "snp_os::fs::rename_with_retry",
    "snp_os::fs::remove_tree_within",
    "snp_os::fs::retire_dir",
    "snp_os::fs::unnamed_temp",
    "snp_os::fs::ensure_private_dir",
    "snp_os::lock::FileLock::try_acquire",
    "snp_os::lock::FileLock::acquire",
];
/// The write types banned outside the writer module.
pub(crate) const BANNED_TYPES: [&str; 1] = ["std::fs::OpenOptions"];

/// The `path`s of a clippy `disallowed-*` list.
fn paths(t: &toml::Table, key: &str) -> BTreeSet<String> {
    t.get(key)
        .and_then(toml::Value::as_array)
        .map_or_default(|a| {
            a.iter()
                .filter_map(|v| match v {
                    toml::Value::String(s) => Some(s.clone()),
                    toml::Value::Table(t) => t.get("path")?.as_str().map(str::to_owned),
                    _ => None,
                })
                .collect()
        })
}

/// The problems of the crate's clippy configuration `local` against the workspace's `workspace`.
pub(crate) fn check_config(workspace: &str, local: &str) -> anyhow::Result<Vec<String>> {
    let w: toml::Table = workspace.parse().context("parsing clippy.toml")?;
    let l: toml::Table = local
        .parse()
        .with_context(|| format!("parsing {CRATE}/clippy.toml"))?;
    let mut v = Vec::new();
    for (key, value) in &w {
        if key == "disallowed-methods" {
            let (want, have) = (value.as_array(), l.get(key).and_then(toml::Value::as_array));
            for entry in want.into_iter().flatten() {
                if !have.is_some_and(|h| h.contains(entry)) {
                    v.push(format!(
                        "{CRATE}/clippy.toml: workspace disallowed-methods entry missing or changed: {entry}"
                    ));
                }
            }
        } else if l.get(key) != Some(value) {
            v.push(format!(
                "{CRATE}/clippy.toml: `{key}` differs from the workspace clippy.toml"
            ));
        }
    }
    let methods = paths(&l, "disallowed-methods");
    for m in BANNED_METHODS {
        if !methods.contains(m) {
            v.push(format!(
                "{CRATE}/clippy.toml: the write ban misses method `{m}`"
            ));
        }
    }
    let types = paths(&l, "disallowed-types");
    for t in BANNED_TYPES {
        if !types.contains(t) {
            v.push(format!(
                "{CRATE}/clippy.toml: the write ban misses type `{t}`"
            ));
        }
    }
    Ok(v)
}

/// Whether `attrs` holds `#[cfg(test)]`.
fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test")
    })
}

/// Counts `expect`/`allow` attributes naming the ban's lints outside `#[cfg(test)]` items, and collects the
/// out-of-line modules declared under `#[cfg(test)]`.
#[derive(Debug, Default)]
struct Scan {
    hits: usize,
    test_mods: Vec<String>,
}

impl Scan {
    fn attrs(&mut self, attrs: &[syn::Attribute]) {
        for a in attrs {
            if !(a.path().is_ident("expect") || a.path().is_ident("allow")) {
                continue;
            }
            if let syn::Meta::List(l) = &a.meta {
                let text = l.tokens.to_string();
                if text.contains("disallowed_methods") || text.contains("disallowed_types") {
                    self.hits = self.hits.saturating_add(1);
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match i {
            syn::Item::Fn(x) => &x.attrs,
            syn::Item::Mod(x) => &x.attrs,
            syn::Item::Impl(x) => &x.attrs,
            syn::Item::Const(x) => &x.attrs,
            syn::Item::Static(x) => &x.attrs,
            syn::Item::Struct(x) => &x.attrs,
            syn::Item::Enum(x) => &x.attrs,
            syn::Item::Trait(x) => &x.attrs,
            syn::Item::Use(x) => &x.attrs,
            _ => &[],
        };
        if is_cfg_test(attrs) {
            if let syn::Item::Mod(m) = i
                && m.content.is_none()
            {
                self.test_mods.push(m.ident.to_string());
            }
            return;
        }
        visit::visit_item(self, i);
    }

    fn visit_attribute(&mut self, i: &'ast syn::Attribute) {
        self.attrs(std::slice::from_ref(i));
    }
}

/// The scan of one source file: `(ban expectations outside test code, test-only child modules)`.
pub(crate) fn scan_source(src: &str) -> anyhow::Result<(usize, Vec<String>)> {
    let file = syn::parse_file(src).context("parsing Rust source")?;
    let mut s = Scan::default();
    s.attrs(&file.attrs);
    for item in &file.items {
        s.visit_item(item);
    }
    Ok((s.hits, s.test_mods))
}

/// The directory a module file's out-of-line children live in (`foo.rs` → `foo/`, `mod.rs`/`lib.rs` → its dir).
fn child_dir(file: &Path) -> PathBuf {
    let parent = file.parent().map_or_default(Path::to_path_buf);
    match file.file_stem().and_then(|s| s.to_str()) {
        Some(stem) if !matches!(stem, "mod" | "lib" | "main") => parent.join(stem),
        _ => parent,
    }
}

/// The problems of the crate's sources below `src`.
pub(crate) fn check_sources(krate: &Path) -> anyhow::Result<Vec<String>> {
    let src = krate.join("src");
    let files = crate::workspace::rust_files(&src)?;
    let mut test_only: BTreeSet<PathBuf> = BTreeSet::new();
    let mut scans = Vec::new();
    for f in &files {
        let text =
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let (hits, mods) = scan_source(&text).with_context(|| f.display().to_string())?;
        let dir = child_dir(f);
        for m in mods {
            test_only.insert(dir.join(format!("{m}.rs")));
            test_only.insert(dir.join(&m).join("mod.rs"));
        }
        scans.push((f.clone(), hits));
    }
    let writer = krate.join(WRITER);
    let mut v = Vec::new();
    for (f, hits) in scans {
        if f == writer || test_only.contains(&f) {
            continue;
        }
        if hits > 0 {
            let rel = crate::workspace::rel(krate, &f);
            v.push(format!(
                "{CRATE}/{rel}: {hits} expectation(s) of clippy::disallowed_methods / disallowed_types outside test \
                 code; only {WRITER} may write (ADR-132: natives write through HookCx::writer)"
            ));
        }
    }
    if !writer.is_file() {
        bail!("{CRATE}/{WRITER} is missing");
    }
    Ok(v)
}

/// Every problem of the write ban, from the workspace root.
pub(crate) fn check(root: &Path) -> anyhow::Result<Vec<String>> {
    let read =
        |p: &Path| std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()));
    let krate = root.join(CRATE);
    let mut v = check_config(
        &read(&root.join("clippy.toml"))?,
        &read(&krate.join("clippy.toml"))?,
    )?;
    v.extend(check_sources(&krate)?);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_that_drops_a_workspace_rule_or_a_ban_fails() {
        let ws = "msrv = \"1.100\"\ndisallowed-methods = [{ path = \"std::env::var\", reason = \"x\" }]\n";
        let bans = BANNED_METHODS
            .iter()
            .map(|m| format!("{{ path = \"{m}\", reason = \"w\" }},"))
            .collect::<Vec<_>>()
            .join("\n");
        let ban = format!(
            "msrv = \"1.100\"\ndisallowed-methods = [\n{{ path = \"std::env::var\", reason = \"x\" }},\n{bans}]\n\
             disallowed-types = [{{ path = \"std::fs::OpenOptions\", reason = \"w\" }}]\n"
        );
        assert_eq!(check_config(ws, &ban).unwrap(), Vec::<String>::new());
        let drifted = ban.replace("msrv = \"1.100\"", "msrv = \"1.99\"");
        assert_eq!(check_config(ws, &drifted).unwrap().len(), 1);
        let dropped = ban.replace("{ path = \"std::env::var\", reason = \"x\" },\n", "");
        assert_eq!(check_config(ws, &dropped).unwrap().len(), 1);
        let no_write = ban.replace("{ path = \"std::fs::write\", reason = \"w\" },\n", "");
        let v = check_config(ws, &no_write).unwrap();
        assert!(v.len() == 1 && v[0].contains("std::fs::write"), "{v:?}");
        let no_types = ban.replace("std::fs::OpenOptions\"", "std::fs::File\"");
        assert_eq!(check_config(ws, &no_types).unwrap().len(), 1);
    }

    #[test]
    fn an_expectation_outside_test_code_is_found() {
        let src = "#[expect(clippy::disallowed_methods, reason = \"r\")]\nfn f() {}\n\
                   #[cfg(test)]\nmod tests { #[expect(clippy::disallowed_methods, reason = \"t\")] fn g() {} }\n\
                   #[cfg(test)]\nmod more;\n\
                   impl S { #[allow(clippy::disallowed_types, reason = \"r\")] fn h() {} }\n\
                   #[expect(clippy::too_many_arguments, reason = \"r\")]\nfn k() {}\n";
        let (hits, mods) = scan_source(src).unwrap();
        assert_eq!(hits, 2);
        assert_eq!(mods, ["more"]);
    }

    #[test]
    fn the_workspace_write_ban_is_in_force() {
        let root = crate::workspace::root().unwrap();
        assert_eq!(check(&root).unwrap(), Vec::<String>::new());
    }

    /// The ban really fires: clippy, run with the crate's clippy.toml on a crate that calls `std::fs::write` and
    /// opens an `OpenOptions`, reports both (a scratch crate outside the workspace, offline, no dependencies).
    #[test]
    fn the_lint_catches_a_raw_write() {
        let root = crate::workspace::root().unwrap();
        let dir = std::env::temp_dir().join(format!("snp-write-ban-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).unwrap_or_default();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::copy(
            root.join(CRATE).join("clippy.toml"),
            dir.join("clippy.toml"),
        )
        .unwrap();
        std::fs::copy(
            root.join("rust-toolchain.toml"),
            dir.join("rust-toolchain.toml"),
        )
        .unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"write-ban-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "//! Probe.\n\n/// A native that bypasses the writer.\npub fn bypass(p: &std::path::Path) -> std::io::Result<()> {\n    \
             std::fs::write(p, b\"x\")?;\n    let _f = std::fs::OpenOptions::new().append(true).open(p)?;\n    \
             let r = std::fs::File::open(p)?;\n    r.set_permissions(r.metadata()?.permissions())?;\n    \
             r.set_modified(std::time::SystemTime::now())?;\n    Ok(())\n}\n",
        )
        .unwrap();
        let out = std::process::Command::new("cargo")
            .args([
                "clippy",
                "--offline",
                "--quiet",
                "--",
                "-D",
                "clippy::disallowed_methods",
                "-D",
                "clippy::disallowed_types",
            ])
            .current_dir(&dir)
            .env("CARGO_TARGET_DIR", dir.join("target"))
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("CARGO_BUILD_TARGET")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        std::fs::remove_dir_all(&dir).unwrap_or_default();
        assert!(
            !out.status.success(),
            "clippy passed a raw write:\n{stderr}"
        );
        assert!(
            stderr.contains("use of a disallowed method `std::fs::write`"),
            "{stderr}"
        );
        assert!(
            stderr.contains("use of a disallowed type `std::fs::OpenOptions`"),
            "{stderr}"
        );
        for m in ["std::fs::File::set_permissions", "std::fs::File::set_modified"] {
            assert!(
                stderr.contains(&format!("use of a disallowed method `{m}`")),
                "{stderr}"
            );
        }
    }
}
