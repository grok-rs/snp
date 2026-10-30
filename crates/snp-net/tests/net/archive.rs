//! Safe extraction against hostile archives (WBS WP-16 acceptance: "archive fixtures with absolute, `..`,
//! drive-letter, escaping-symlink, chained-symlink (`x -> .`, `y -> x/..`), hardlink-outside, device/FIFO and setuid
//! entries are rejected/neutralised; a kill during extraction leaves only a .part directory"; ADR-077).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use snp_net::NetError;
use snp_net::archive::{self, Extract};

/// tar entry types.
const REG: u8 = b'0';
const HARD: u8 = b'1';
const SYM: u8 = b'2';
const CHR: u8 = b'3';
const BLK: u8 = b'4';
const DIR: u8 = b'5';
const FIFO: u8 = b'6';

/// One raw tar entry (any name and link bytes: the `tar` crate's safe setters would refuse the hostile ones).
struct E<'a> {
    name: &'a [u8],
    kind: u8,
    mode: u32,
    data: &'a [u8],
    link: &'a [u8],
}

fn file<'a>(name: &'a str, mode: u32, data: &'a [u8]) -> E<'a> {
    E {
        name: name.as_bytes(),
        kind: REG,
        mode,
        data,
        link: b"",
    }
}
fn dir(name: &str, mode: u32) -> E<'_> {
    E {
        name: name.as_bytes(),
        kind: DIR,
        mode,
        data: b"",
        link: b"",
    }
}
fn sym<'a>(name: &'a str, target: &'a str) -> E<'a> {
    E {
        name: name.as_bytes(),
        kind: SYM,
        mode: 0o777,
        data: b"",
        link: target.as_bytes(),
    }
}
fn hard<'a>(name: &'a str, target: &'a str) -> E<'a> {
    hard_mode(name, target, 0o644)
}
fn hard_mode<'a>(name: &'a str, target: &'a str, mode: u32) -> E<'a> {
    E {
        name: name.as_bytes(),
        kind: HARD,
        mode,
        data: b"",
        link: target.as_bytes(),
    }
}
fn special(name: &str, kind: u8) -> E<'_> {
    E {
        name: name.as_bytes(),
        kind,
        mode: 0o644,
        data: b"",
        link: b"",
    }
}

/// An uncompressed tar stream.
fn tar(entries: &[E<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(u64::try_from(e.data.len()).unwrap());
        h.set_mode(e.mode);
        h.set_entry_type(tar::EntryType::new(e.kind));
        h.set_mtime(0);
        h.set_link_name_literal(e.link).unwrap();
        let bytes = h.as_mut_bytes();
        bytes[..100].fill(0);
        for (slot, b) in bytes.iter_mut().zip(e.name) {
            *slot = *b;
        }
        h.set_cksum();
        out.extend_from_slice(h.as_bytes());
        out.extend_from_slice(e.data);
        let pad = (512 - e.data.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0_u8, pad));
    }
    out.extend(std::iter::repeat_n(0_u8, 1024));
    out
}

fn gz(raw: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(raw).unwrap();
    enc.finish().unwrap()
}

const OPTS: Extract = Extract {
    strip_components: 0,
    allow_symlinks_inside: true,
    max_total_bytes: 1 << 20,
};

/// A workspace with the archive written to `a.tar.gz` and the destination `out/dest`.
struct Case {
    dir: tempfile::TempDir,
}

impl Case {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        Self { dir }
    }
    fn dest(&self) -> PathBuf {
        self.dir.path().join("out/dest")
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
    fn tar_gz(&self, entries: &[E<'_>], o: &Extract) -> Result<(), NetError> {
        let a = self.write("a.tar.gz", &gz(&tar(entries)));
        archive::tar_gz(&a, &self.dest(), o)
    }
    /// Names left in `out/` (the destination and any `.part` directory).
    fn out_names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.dir.path().join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
    /// A refused archive leaves nothing behind and never touches the workspace outside `out/`.
    fn assert_refused(&self, r: Result<(), NetError>, what: &str) {
        let e = r.expect_err(what);
        assert!(matches!(e, NetError::UnsafeEntry(_)), "{what}: {e:?}");
        assert!(
            self.out_names().is_empty(),
            "{what}: {:?}",
            self.out_names()
        );
        assert!(!self.dir.path().join("evil").exists(), "{what}");
        assert!(!self.dir.path().join("x").exists(), "{what}");
    }
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_regular_archive_extracts_with_strip_links_and_modes() {
    let c = Case::new();
    let entries = [
        dir("node-v1/", 0o755),
        dir("node-v1/bin/", 0o755),
        file("node-v1/bin/node", 0o755, b"#!/bin/sh\necho node\n"),
        dir("node-v1/lib/npm/", 0o755),
        file("node-v1/lib/npm/cli.js", 0o644, b"cli"),
        sym("node-v1/bin/npm", "../lib/npm/cli.js"),
        // The link's mode applies to the shared inode (CPython `tarfile` `chmod`s it after `os.link`).
        hard_mode("node-v1/bin/node2", "node-v1/bin/node", 0o750),
        // A parent without its own entry.
        file("node-v1/share/doc/README", 0o644, b"readme"),
        // A read-only directory: its mode is applied after its content was written.
        dir("node-v1/ro/", 0o555),
        file("node-v1/ro/f", 0o444, b"f"),
        // Duplicates: the later entry wins.
        file("node-v1/dup", 0o644, b"first"),
        file("node-v1/dup", 0o644, b"second"),
    ];
    c.tar_gz(
        &entries,
        &Extract {
            strip_components: 1,
            ..OPTS
        },
    )
    .unwrap();
    let d = c.dest();
    assert_eq!(c.out_names(), ["dest"]);
    assert_eq!(
        std::fs::read(d.join("bin/node")).unwrap(),
        b"#!/bin/sh\necho node\n"
    );
    assert_eq!(
        std::fs::read(d.join("bin/node2")).unwrap(),
        b"#!/bin/sh\necho node\n"
    );
    assert_eq!(
        std::fs::read(d.join("share/doc/README")).unwrap(),
        b"readme"
    );
    assert_eq!(std::fs::read(d.join("dup")).unwrap(), b"second");
    assert_eq!(std::fs::read(d.join("ro/f")).unwrap(), b"f");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        assert_eq!(
            std::fs::read_link(d.join("bin/npm")).unwrap(),
            Path::new("../lib/npm/cli.js")
        );
        assert_eq!(std::fs::read(d.join("bin/npm")).unwrap(), b"cli");
        assert_eq!(mode(&d.join("bin/node")), 0o750);
        assert_eq!(mode(&d.join("bin/node2")), 0o750);
        assert_eq!(mode(&d.join("lib/npm/cli.js")), 0o644);
        assert_eq!(mode(&d.join("ro")), 0o555);
        // Directories without an entry (and the root) get `0o777 & !umask`, as `os.makedirs` gives them.
        let probe = c.dir.path().join("umask-probe");
        std::fs::create_dir_all(&probe).unwrap();
        let implicit = mode(&probe);
        assert_eq!(mode(&d.join("share")), implicit);
        assert_eq!(mode(&d.join("share/doc")), implicit);
        assert_eq!(mode(&d), implicit);
        assert_eq!(
            std::fs::metadata(d.join("bin/node")).unwrap().ino(),
            std::fs::metadata(d.join("bin/node2")).unwrap().ino()
        );
        // The read-only directory does not block cleanup by the caller.
        std::fs::set_permissions(d.join("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn absolute_dotdot_and_drive_names_are_refused() {
    for name in [
        "/evil",
        "/tmp/evil",
        "a/../../evil",
        "../evil",
        "..",
        "C:/evil",
        "c:evil",
        "\\evil",
    ] {
        let c = Case::new();
        let r = c.tar_gz(
            &[file("ok", 0o644, b"ok"), file(name, 0o644, b"evil")],
            &OPTS,
        );
        c.assert_refused(r, name);
    }
}

#[test]
fn escaping_and_absolute_symlinks_are_refused() {
    for (link, target) in [
        ("l", "../../evil"),
        ("a/l", "../.."),
        ("l", "/etc/passwd"),
        ("l", "C:\\Windows"),
        ("l", ""),
    ] {
        let c = Case::new();
        let r = c.tar_gz(&[dir("a/", 0o755), sym(link, target)], &OPTS);
        c.assert_refused(r, &format!("{link} -> {target}"));
    }
}

#[test]
fn chained_symlinks_are_refused() {
    // `x -> .` alone stays inside the tree.
    let c = Case::new();
    c.tar_gz(&[sym("x", ".")], &OPTS).unwrap();
    // … but `y -> x/..` passes through `x` and would leave it.
    for entries in [
        vec![sym("x", "."), sym("y", "x/..")],
        vec![sym("y", "x/.."), sym("x", ".")],
        vec![sym("a", "b"), sym("b", "f"), file("f", 0o644, b"f")],
        vec![dir("d/", 0o755), sym("d/l", "."), sym("d/l/m", "z")],
        vec![sym("self", "self")],
    ] {
        let c = Case::new();
        let r = c.tar_gz(&entries, &OPTS);
        c.assert_refused(r, "chain");
    }
}

/// The ADR-077 vector through a case variant: on a case-insensitive filesystem (macOS, Windows) `x` opens `X`, so
/// name equality with the archive's symlinks cannot see the chain. Every `..` must step back over a real directory.
#[test]
fn case_variant_chains_are_refused() {
    for entries in [
        vec![sym("X", "."), sym("y", "x/..")],
        vec![sym("y", "x/.."), sym("X", ".")],
        vec![dir("d/", 0o755), sym("d/x", ".."), sym("y", "d/X/..")],
        vec![dir("d/", 0o755), sym("y", "d/X/.."), sym("d/x", "..")],
        // `..` over a name that is no directory of the tree at all (a file, nothing).
        vec![file("f", 0o644, b"f"), sym("y", "f/../f")],
        vec![file("f", 0o644, b"f"), sym("y", "nothing/../f")],
    ] {
        let c = Case::new();
        let r = c.tar_gz(&entries, &OPTS);
        c.assert_refused(r, "case-variant chain");
    }
    // `..` over real directories (explicit or implicit) still works.
    let c = Case::new();
    c.tar_gz(
        &[
            file("a/b/f", 0o644, b"f"),
            sym("a/l", "b/../b/f"),
            sym("c/l", "../a/b/f"),
        ],
        &OPTS,
    )
    .unwrap();
    assert_eq!(std::fs::read(c.dest().join("a/l")).unwrap(), b"f");
    assert_eq!(std::fs::read(c.dest().join("c/l")).unwrap(), b"f");
}

/// A GNU long-name record (`././@LongLink`) far beyond any path, gzip-bombed: refused as too large before the `tar`
/// crate buffers it (it reads such records whole into memory).
#[test]
fn a_huge_long_name_record_is_too_large() {
    let case = Case::new();
    let mib: u64 = 16;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut header = tar::Header::new_gnu();
    for (slot, b) in header.as_mut_bytes().iter_mut().zip(b"././@LongLink") {
        *slot = *b;
    }
    header.set_size(mib << 20);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_cksum();
    enc.write_all(header.as_bytes()).unwrap();
    let chunk = vec![b'a'; 1 << 20];
    for _ in 0..mib {
        enc.write_all(&chunk).unwrap();
    }
    enc.write_all(&tar(&[file("f", 0o644, b"f")])).unwrap();
    let path = case.write("a.tar.gz", &enc.finish().unwrap());
    let r = archive::tar_gz(&path, &case.dest(), &OPTS);
    assert!(
        matches!(&r, Err(NetError::TooLarge { limit, .. }) if *limit == 1 << 20),
        "{r:?}"
    );
    assert_eq!(case.out_names(), Vec::<String>::new());
}

/// Deferred symlinks (name + target, up to 4096 bytes each) are charged to the metadata budget (64 MiB): an archive
/// of thousands of maximal ones is refused instead of growing without bound.
#[test]
fn the_recorded_metadata_is_capped() {
    let c = Case::new();
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let target: String = std::iter::repeat_n('t', 4000).collect();
    let pad: String = std::iter::repeat_n('x', 4000).collect();
    for i in 0..6000 {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_mode(0o777);
        h.set_size(0);
        let name = format!("l{i:05}{pad}");
        b.append_link(&mut h, &name, &target).unwrap();
    }
    let a = c.write("a.tar.gz", &b.into_inner().unwrap().finish().unwrap());
    let r = archive::tar_gz(&a, &c.dest(), &OPTS);
    assert!(
        matches!(&r, Err(NetError::TooLarge { limit, .. }) if *limit == 64 << 20),
        "{r:?}"
    );
    assert_eq!(c.out_names(), Vec::<String>::new());
}

#[test]
fn symlinks_can_be_forbidden() {
    let c = Case::new();
    let r = c.tar_gz(
        &[file("f", 0o644, b"f"), sym("l", "f")],
        &Extract {
            allow_symlinks_inside: false,
            ..OPTS
        },
    );
    c.assert_refused(r, "no symlinks");
}

#[test]
fn hardlinks_only_to_extracted_regular_files() {
    for (name, entries) in [
        ("outside", vec![hard("h", "/etc/passwd")]),
        ("dotdot", vec![hard("h", "../x")]),
        (
            "not yet extracted",
            vec![hard("h", "f"), file("f", 0o644, b"f")],
        ),
        (
            "to a symlink",
            vec![file("f", 0o644, b"f"), sym("l", "f"), hard("h", "l")],
        ),
        ("to a directory", vec![dir("d/", 0o755), hard("h", "d")]),
    ] {
        let c = Case::new();
        let r = c.tar_gz(&entries, &OPTS);
        c.assert_refused(r, name);
    }
}

#[test]
fn devices_and_fifos_are_refused() {
    for kind in [CHR, BLK, FIFO, b'V', b'Z'] {
        let c = Case::new();
        let r = c.tar_gz(&[file("ok", 0o644, b"ok"), special("dev", kind)], &OPTS);
        c.assert_refused(r, &format!("type {}", char::from(kind)));
    }
}

#[cfg(unix)]
#[test]
fn setuid_setgid_and_sticky_bits_are_stripped() {
    let c = Case::new();
    c.tar_gz(
        &[
            file("suid", 0o4755, b"x"),
            file("sgid", 0o2755, b"x"),
            dir("sticky/", 0o1777),
            dir("gdir/", 0o2775),
        ],
        &OPTS,
    )
    .unwrap();
    let d = c.dest();
    assert_eq!(mode(&d.join("suid")), 0o755);
    assert_eq!(mode(&d.join("sgid")), 0o755);
    assert_eq!(mode(&d.join("sticky")), 0o777);
    assert_eq!(mode(&d.join("gdir")), 0o775);
}

#[test]
fn the_total_size_is_capped() {
    let c = Case::new();
    let big = vec![b'z'; 600];
    let r = c.tar_gz(
        &[file("a", 0o644, &big), file("b", 0o644, &big)],
        &Extract {
            max_total_bytes: 1000,
            ..OPTS
        },
    );
    assert!(
        matches!(r, Err(NetError::TooLarge { limit: 1000, .. })),
        "{r:?}"
    );
    assert_eq!(c.out_names(), Vec::<String>::new());
}

#[test]
fn deep_names_are_refused() {
    // GNU long names (written by `tar::Builder`) can be arbitrarily deep; the extractor caps the depth.
    let deep: String = std::iter::repeat_n("d/", 200).collect::<String>() + "f";
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    builder.append_data(&mut header, &deep, &b"x"[..]).unwrap();
    let raw = builder.into_inner().unwrap();
    let case = Case::new();
    let path = case.write("a.tar.gz", &gz(&raw));
    let result = archive::tar_gz(&path, &case.dest(), &OPTS);
    case.assert_refused(result, "deep");
}

#[test]
fn an_existing_destination_is_never_overwritten() {
    let c = Case::new();
    std::fs::create_dir_all(c.dest()).unwrap();
    std::fs::write(c.dest().join("keep"), b"keep").unwrap();
    let r = c.tar_gz(&[file("f", 0o644, b"f")], &OPTS);
    assert!(r.is_err());
    assert_eq!(c.out_names(), ["dest"]);
    assert_eq!(std::fs::read(c.dest().join("keep")).unwrap(), b"keep");
    assert!(!c.dest().join("f").exists());
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    let c = Case::new();
    let a = c.write("junk.tar.gz", b"\x1f\x8b\x08\x00garbage");
    assert!(archive::tar_gz(&a, &c.dest(), &OPTS).is_err());
    assert!(archive::zip(&a, &c.dest(), &OPTS).is_err());
    assert_eq!(c.out_names(), Vec::<String>::new());
}

/// zip archives built with the `zip` crate; hostile names are written raw.
fn zip_bytes(entries: &[(&str, ZipKind<'_>)]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, kind) in entries {
        match kind {
            ZipKind::File(mode, data) => {
                w.start_file(*name, SimpleFileOptions::default().unix_permissions(*mode))
                    .unwrap();
                w.write_all(data).unwrap();
            }
            ZipKind::Dir => w
                .add_directory(*name, SimpleFileOptions::default())
                .unwrap(),
            ZipKind::Symlink(target) => w
                .add_symlink(*name, *target, SimpleFileOptions::default())
                .unwrap(),
            ZipKind::RawMode(_) => w.start_file(*name, SimpleFileOptions::default()).unwrap(),
        }
    }
    let mut bytes = w.finish().unwrap().into_inner();
    for (name, kind) in entries {
        if let ZipKind::RawMode(mode) = kind {
            patch_zip_mode(&mut bytes, name, *mode);
        }
    }
    bytes
}

enum ZipKind<'a> {
    File(u32, &'a [u8]),
    Dir,
    Symlink(&'a str),
    /// A regular entry whose central-directory Unix mode is patched afterwards (FIFOs, devices: the writer API
    /// cannot express them).
    RawMode(u32),
}

/// Sets the Unix mode of the central-directory entry `name` (external attributes, host = Unix).
fn patch_zip_mode(zip: &mut [u8], name: &str, mode: u32) {
    const CENTRAL: [u8; 4] = [0x50, 0x4b, 1, 2];
    let mut i = 0;
    while i + 46 <= zip.len() {
        if zip[i..i + 4] == CENTRAL {
            let name_len = usize::from(u16::from_le_bytes([zip[i + 28], zip[i + 29]]));
            if zip.get(i + 46..i + 46 + name_len) == Some(name.as_bytes()) {
                zip[i + 5] = 3;
                for (slot, b) in zip[i + 38..i + 42]
                    .iter_mut()
                    .zip((mode << 16).to_le_bytes())
                {
                    *slot = b;
                }
                return;
            }
        }
        i += 1;
    }
    panic!("no central directory entry {name}");
}

#[test]
fn zip_regular_and_hostile_entries() {
    let c = Case::new();
    let a = c.write(
        "ok.zip",
        &zip_bytes(&[
            ("go/", ZipKind::Dir),
            ("go/bin/go.exe", ZipKind::File(0o755, b"go")),
            ("go/VERSION", ZipKind::File(0o644, b"go1.23")),
            ("go/link", ZipKind::Symlink("VERSION")),
            ("go/suid", ZipKind::File(0o4755, b"s")),
        ]),
    );
    archive::zip(
        &a,
        &c.dest(),
        &Extract {
            strip_components: 1,
            ..OPTS
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(c.dest().join("bin/go.exe")).unwrap(), b"go");
    assert_eq!(std::fs::read(c.dest().join("VERSION")).unwrap(), b"go1.23");
    #[cfg(unix)]
    {
        assert_eq!(std::fs::read(c.dest().join("link")).unwrap(), b"go1.23");
        assert_eq!(mode(&c.dest().join("bin/go.exe")), 0o755);
        assert_eq!(mode(&c.dest().join("suid")), 0o755);
    }

    for (what, entries) in [
        ("dotdot", vec![("../evil", ZipKind::File(0o644, b"e"))]),
        ("absolute", vec![("/evil", ZipKind::File(0o644, b"e"))]),
        (
            "backslash",
            vec![("a\\..\\..\\evil", ZipKind::File(0o644, b"e"))],
        ),
        ("drive", vec![("C:/evil", ZipKind::File(0o644, b"e"))]),
        ("escaping link", vec![("l", ZipKind::Symlink("../evil"))]),
        ("fifo", vec![("p", ZipKind::RawMode(0o010_644))]),
        ("char device", vec![("c", ZipKind::RawMode(0o020_644))]),
        ("socket", vec![("s", ZipKind::RawMode(0o140_644))]),
    ] {
        let c = Case::new();
        let a = c.write("bad.zip", &zip_bytes(&entries));
        let r = archive::zip(&a, &c.dest(), &OPTS);
        c.assert_refused(r, what);
    }
}

#[cfg(feature = "xz")]
#[test]
fn tar_xz_extracts() {
    use liblzma::write::XzEncoder;
    let mut enc = XzEncoder::new(Vec::new(), 1);
    enc.write_all(&tar(&[dir("t/", 0o755), file("t/x", 0o644, b"xz")]))
        .unwrap();
    let c = Case::new();
    let a = c.write("a.tar.xz", &enc.finish().unwrap());
    archive::tar_xz(
        &a,
        &c.dest(),
        &Extract {
            strip_components: 1,
            ..OPTS
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(c.dest().join("x")).unwrap(), b"xz");
}

#[cfg(not(feature = "xz"))]
#[test]
fn tar_xz_without_the_feature_is_unsupported() {
    let c = Case::new();
    let a = c.write("a.tar.xz", b"");
    let e = archive::tar_xz(&a, &c.dest(), &OPTS).unwrap_err();
    assert!(
        matches!(&e, NetError::Io(io) if io.kind() == std::io::ErrorKind::Unsupported),
        "{e:?}"
    );
    assert_eq!(c.out_names(), Vec::<String>::new());
}

/// The vendored ruby helpers extract (rbenv carries `bin/rbenv -> ../libexec/rbenv`).
#[test]
fn the_embedded_ruby_helpers_extract() {
    use snp_net::tools::{self, Tool};
    for tool in [Tool::Rbenv, Tool::RubyBuild, Tool::RubyDownload] {
        let c = Case::new();
        let url = tools::resolve(tool, "default", tools::host_target(), &NoFetch)
            .unwrap()
            .url;
        let a = c.write("a.tar.gz", tools::embedded(&url).unwrap());
        archive::tar_gz(
            &a,
            &c.dest(),
            &Extract {
                max_total_bytes: 64 << 20,
                ..OPTS
            },
        )
        .unwrap();
        assert_eq!(c.out_names(), ["dest"]);
    }
}

/// A `Fetch` that must never be called.
#[derive(Debug)]
struct NoFetch;

impl snp_net::Fetch for NoFetch {
    fn get_text(&self, url: &str, _max: u64) -> Result<String, NetError> {
        Err(NetError::Insecure(format!("unexpected fetch of {url}")))
    }
    fn get_verified(
        &self,
        url: &str,
        _sum: &snp_net::Checksum,
        _dest_dir: &Path,
        _file_name: &str,
        _cancel: &snp_core::cancel::CancelToken,
    ) -> Result<PathBuf, NetError> {
        Err(NetError::Insecure(format!("unexpected fetch of {url}")))
    }
}

/// `SNP_NET_TEST_KILL_CHILD=<fifo>\n<dest>`: the child half of the kill test (extracts from a FIFO that never ends).
#[test]
fn kill_child_helper() {
    #[expect(clippy::disallowed_methods, reason = "test child protocol")]
    let Ok(spec) = std::env::var("SNP_NET_TEST_KILL_CHILD") else {
        return;
    };
    let (fifo, dest) = spec.split_once('\n').unwrap();
    let r = archive::tar_gz(Path::new(fifo), Path::new(dest), &OPTS);
    panic!("the child should have been killed while blocked, got {r:?}");
}

/// A process killed during extraction leaves only the `.part` directory: `dest` never appears half-written.
#[cfg(unix)]
#[test]
fn a_kill_during_extraction_leaves_only_a_part_directory() {
    use std::time::{Duration, Instant};
    let c = Case::new();
    let fifo = c.dir.path().join("archive.fifo");
    // mkfifo(1) is POSIX (rustix has no mknodat on every Unix).
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "archive::kill_child_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            "SNP_NET_TEST_KILL_CHILD",
            format!("{}\n{}", fifo.display(), c.dest().display()),
        )
        .stdout(std::fs::File::create(c.dir.path().join("child.log")).unwrap())
        .stderr(std::fs::File::create(c.dir.path().join("child.err")).unwrap())
        .spawn()
        .unwrap();
    // Feed the first entries and part of a large one (gzip stream sync-flushed), then stall: the child blocks in the
    // middle of `a/big`.
    let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    let big = vec![b'2'; 1 << 19];
    let full = tar(&[
        dir("a/", 0o755),
        file("a/first", 0o644, b"first"),
        file("a/big", 0o644, &big),
    ]);
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&full[..300 * 1024]).unwrap();
    enc.flush().unwrap();
    w.write_all(enc.get_ref()).unwrap();
    w.flush().unwrap();
    let start = Instant::now();
    let part = loop {
        let parts: Vec<_> = c
            .out_names()
            .into_iter()
            .filter(|n| n.starts_with("dest.part-"))
            .collect();
        if let Some(p) = parts.first() {
            let p = c.dir.path().join("out").join(p);
            if p.join("a/first").exists() {
                break p;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the child never started extracting: {:?} {} {}",
            c.out_names(),
            std::fs::read_to_string(c.dir.path().join("child.log")).unwrap_or_default(),
            std::fs::read_to_string(c.dir.path().join("child.err")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    drop(w);
    let names = c.out_names();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with("dest.part-"), "{names:?}");
    assert!(!c.dest().exists());
    assert_eq!(std::fs::read(part.join("a/first")).unwrap(), b"first");
}
