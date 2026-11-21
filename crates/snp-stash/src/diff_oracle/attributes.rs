//! ADR-133 (b): a conservative scan of the attributes files for the attributes that change a checkout's bytes or
//! `git diff`'s verdict.
//!
//! Only `convert.c` `convert_attrs` matters there: it reads `text`, `crlf`, `eol`, `ident`, `filter` and
//! `working-tree-encoding` (and the `core.autocrlf` / `core.eol` keys). Every other attribute (`diff`, `merge`,
//! `whitespace`, `export-*`, `linguist-*`, custom names) changes at most how a non-empty diff is printed. So instead
//! of matching patterns, the scan refuses a file in which any token sets or values one of the six; the unset (`-x`)
//! and unspecified (`!x`) forms convert nothing and are allowed. A macro that sets one of them contains the token
//! (git's only builtin macro, `binary`, unsets `text`), and a pattern spelled like one of them is a false refusal,
//! which is safe.

use std::io::Read;
use std::path::Path;

use super::{Kind, St};

/// git ignores an attributes file of this size or more (`ATTR_MAX_FILE_SIZE`, since the security releases of January
/// 2023; older gits read it): such a file is refused either way.
pub(super) const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Whether `text=auto` may be allowed at all (ADR-133): set because the attribute × blob-content matrix (the test
/// `allowed_attributes_check_out_blob_bytes_that_read_back_clean` below) found every allowed checkout, `text=auto`
/// included, wrote the blob's bytes and read back clean on git 2.18.5, 2.43.0 and 2.56.0; the git-version axis of CI
/// re-runs it on every pinned git. Each run still needs `core.autocrlf` off, `core.eol` unset, `lf` or `native`, and
/// a platform other than Windows.
pub(super) const TEXT_AUTO_MATRIX_PASSED: bool = true;

/// The attributes `convert_attrs` reads.
const CONVERTING: [&[u8]; 6] = [
    b"text",
    b"crlf",
    b"eol",
    b"ident",
    b"filter",
    b"working-tree-encoding",
];

/// How much of a token is kept: more than the longest `name=` above, and more than `text=auto`.
const PREFIX: usize = 32;

/// The token separators: git's blank set `" \t\r\n"` (`attr.c`), plus the double quote (a C-quoted pattern may be
/// followed by an attribute with no blank in between) and NUL (git's line ends there). Extra separators can only split
/// a token git reads as one, which keeps every converting token's own prefix intact: the scan stays an
/// over-approximation.
const fn is_separator(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'"' | 0)
}

/// A streaming tokenizer over one file: at most [`PREFIX`] bytes of the current token and its full length.
#[derive(Debug)]
struct Scanner {
    prefix: [u8; PREFIX],
    len: usize,
    text_auto: bool,
    converts: bool,
}

impl Scanner {
    const fn new(text_auto: bool) -> Self {
        Self {
            prefix: [0; PREFIX],
            len: 0,
            text_auto,
            converts: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if is_separator(b) {
                self.end_token();
            } else {
                if let Some(slot) = self.prefix.get_mut(self.len) {
                    *slot = b;
                }
                self.len = self.len.saturating_add(1);
            }
        }
    }

    fn end_token(&mut self) {
        if self.len > 0 {
            let kept = self.prefix.get(..self.len.min(PREFIX)).unwrap_or_default();
            self.converts |= token_converts(kept, self.len, self.text_auto);
            self.len = 0;
        }
    }
}

/// Whether a token (its first bytes `kept`, `len` bytes in all) sets or values a converting attribute: `name` alone
/// or `name=<anything>`. `text=auto` exactly is allowed when `text_auto` holds.
fn token_converts(kept: &[u8], len: usize, text_auto: bool) -> bool {
    if text_auto && len == b"text=auto".len() && kept == b"text=auto" {
        return false;
    }
    CONVERTING.iter().any(|name| {
        (len == name.len() && kept == *name)
            || kept
                .strip_prefix(*name)
                .is_some_and(|rest| rest.first() == Some(&b'='))
    })
}

/// Whether `bytes` (a whole attributes file) sets or values no converting attribute.
#[cfg(test)]
pub(super) fn text_plain(bytes: &[u8], text_auto: bool) -> bool {
    let mut s = Scanner::new(text_auto);
    s.feed(bytes);
    s.end_token();
    !s.converts
}

/// Whether the file at `path` can be read whole and sets or values no converting attribute; a read error, or
/// [`MAX_FILE_SIZE`] bytes or more, is a refusal.
fn file_plain(path: &Path, text_auto: bool) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut s = Scanner::new(text_auto);
    let mut buf = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total = total.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
                if total >= MAX_FILE_SIZE {
                    return false;
                }
                s.feed(buf.get(..n).unwrap_or_default());
                if s.converts {
                    return false;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    s.end_token();
    !s.converts
}

/// ADR-133 (b) for one attributes input of the snapshot (`lstat`, `stat`): missing, or a regular file (not a
/// symbolic link: git ≥ 2.32 ignores a symlinked `.gitattributes`, older gits follow it) under [`MAX_FILE_SIZE`]
/// whose tokens convert nothing. An unreadable file (git warns and ignores it), a directory or anything else refuses.
pub(super) fn input_plain(path: &Path, (l, s): (St, St), text_auto: bool) -> bool {
    match l.kind {
        Kind::Missing => s.kind == Kind::Missing,
        Kind::File => s.kind == Kind::File && s.size < MAX_FILE_SIZE && file_plain(path, text_auto),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six attributes, set or valued, refuse; unset and unspecified forms and every other attribute do not.
    #[test]
    fn converting_tokens_refuse_and_others_do_not() {
        for bad in [
            "* text\n",
            "*.txt text=auto\n",
            "*.txt text=\n",
            "*.txt text=false\n",
            "*.sh eol=lf\n",
            "*.bat eol=crlf\n",
            "* crlf\n",
            "* crlf=input\n",
            "*.c ident\n",
            "*.bin filter=lfs diff=lfs merge=lfs -text\n",
            "*.x filter\n",
            "*.utf16 working-tree-encoding=UTF-16LE\n",
            "[attr]norm text eol=lf\n",
            "\"a b\" text\n",
            "\"quoted\"text\n",
            "x\ttext\r\n",
            "x text",
            "x -diff text=\"auto\"\n",
            "x text\0junk\n",
            "\u{feff}* text\n",
            "# a comment naming text counts too\n",
            // a pattern named like an attribute: a false refusal
            "text -diff\n",
        ] {
            assert!(!text_plain(bad.as_bytes(), false), "{bad:?}");
        }
        for ok in [
            "",
            "*.png binary\n",
            "*.png -diff\n",
            "*.min.js -diff linguist-generated\n",
            "*.c whitespace=tab-in-indent diff=cpp\n",
            "* -text\n",
            "* !text !eol !crlf !filter !ident !working-tree-encoding\n",
            "* -text -eol -crlf -filter -ident -working-tree-encoding\n",
            "* texts=1 textual eol_x filtered identity\n",
            "*.x export-ignore merge=union conflict-marker-size=10 delta\n",
            "[attr]mine -diff -merge\nfoo mine\n",
            "\u{feff}*.png binary\r\n",
        ] {
            assert!(text_plain(ok.as_bytes(), false), "{ok:?}");
            assert!(text_plain(ok.as_bytes(), true), "{ok:?}");
        }
    }

    /// `text=auto` exactly is allowed only when the caller says so; every other `text` form still refuses.
    #[test]
    fn text_auto_is_allowed_only_when_asked() {
        for ok in [
            "* text=auto\n",
            "* text=auto\r\n*.png binary\n",
            "\"a\"text=auto",
        ] {
            assert!(text_plain(ok.as_bytes(), true), "{ok:?}");
            assert!(!text_plain(ok.as_bytes(), false), "{ok:?}");
        }
        for bad in [
            "* text=auto eol=lf\n",
            "* text=autox\n",
            "* text=Auto\n",
            "* text\n",
            "* crlf=auto\n",
            "* text=auto\n*.c text\n",
        ] {
            assert!(!text_plain(bad.as_bytes(), true), "{bad:?}");
        }
    }

    /// A long token keeps its prefix: `filter=` followed by 10 000 bytes still refuses, a long harmless one does not.
    #[test]
    fn long_tokens_are_judged_by_their_prefix() {
        let long = format!(
            "* filter={}\n",
            std::iter::repeat_n("x", 10_000).collect::<String>()
        );
        assert!(!text_plain(long.as_bytes(), false));
        let harmless = format!(
            "* {}\n",
            std::iter::repeat_n("text", 5_000).collect::<String>()
        );
        assert!(text_plain(harmless.as_bytes(), false));
        let line = format!(
            "{} text\n",
            std::iter::repeat_n("*", 5_000).collect::<String>()
        );
        assert!(
            !text_plain(line.as_bytes(), false),
            "a line git ignores is scanned anyway"
        );
    }

    /// The file reader agrees with the in-memory scan across its 16 KiB reads (a token split over two reads).
    #[test]
    fn files_are_scanned_across_reads() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("attrs");
        for at in [16 * 1024 - 2, 16 * 1024 - 1, 16 * 1024, 16 * 1024 + 1] {
            let mut body = vec![b'#'; at];
            body.extend_from_slice(b" text\n");
            std::fs::write(&p, &body).unwrap();
            assert!(!file_plain(&p, false), "{at}");
            let mut ok = vec![b'#'; at];
            ok.extend_from_slice(b" -text\n");
            std::fs::write(&p, &ok).unwrap();
            assert!(file_plain(&p, false), "{at}");
        }
        assert!(!file_plain(&d.path().join("missing"), false));
    }

    /// The attribute × blob-content matrix behind [`TEXT_AUTO_MATRIX_PASSED`], against real git: the git on `PATH`
    /// and every `bin` directory of `SNP_TEST_GIT_MATRIX` (`:`-separated; run locally with 2.18.5 and 2.56.0 beside
    /// the system 2.43). For every attributes line the scanner allows (with `text=auto`), under `core.eol` unset,
    /// `lf` and `native`, a checkout of index blobs (LF, CRLF, mixed, lone CR, binary-looking, BOM-prefixed, without
    /// a final newline, empty) over a dirty worktree writes each blob's exact bytes, and the parity `git diff` right
    /// after it (racily clean, so git reads every file) prints nothing. A refused line (`text eol=crlf`) does change
    /// the bytes, so the matrix can tell.
    #[cfg(unix)]
    #[test]
    fn allowed_attributes_check_out_blob_bytes_that_read_back_clean() {
        use std::fmt::Write as _;
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        const CONTENTS: [(&str, &[u8]); 9] = [
            ("lf", b"a\nb\n"),
            ("crlf", b"a\r\nb\r\n"),
            ("mixed", b"a\r\nb\nc\r\n"),
            ("lone-cr", b"a\rb\n"),
            ("binary", b"a\0b\r\nc\n"),
            ("bom-crlf", b"\xEF\xBB\xBFa\r\nb\r\n"),
            ("bom-lf", b"\xEF\xBB\xBFa\nb\n"),
            ("no-final-newline", b"a\r\nb"),
            ("empty", b""),
        ];
        const ATTRS: [&str; 11] = [
            "",
            "binary",
            "-diff",
            "linguist-generated diff=python",
            "-text",
            "!text",
            "-crlf -eol -ident -filter",
            "text=auto",
            "text=auto -diff",
            "whitespace=cr-at-eol merge=union",
            "text eol=crlf",
        ];
        let process = snp_core::env::Env::capture_process();
        let mut gits = vec![std::path::PathBuf::from("git")];
        if let Some(dirs) = process.get("SNP_TEST_GIT_MATRIX") {
            gits.extend(std::env::split_paths(dirs).map(|d| d.join("git")));
        }
        let path = process
            .get("PATH")
            .map_or_else(std::ffi::OsString::new, std::ffi::OsStr::to_os_string);
        let mut checked = 0_usize;
        for exe in &gits {
            for attrs in ATTRS {
                let allowed = text_plain(format!("f* {attrs}\n").as_bytes(), true);
                let eols: &[&str] = if attrs.contains("text=auto") {
                    &["", "lf", "native", "LF"]
                } else {
                    &[""]
                };
                for eol in eols {
                    let d = tempfile::tempdir().unwrap();
                    let home = d.path().join("home");
                    let root = d.path().join("repo");
                    std::fs::create_dir_all(&home).unwrap();
                    std::fs::create_dir_all(&root).unwrap();
                    let git = |args: &[&str], stdin: Option<&[u8]>| {
                        let mut c = Command::new(exe);
                        c.args(args)
                            .current_dir(&root)
                            .env_clear()
                            .env("PATH", &path)
                            .env("HOME", &home)
                            .env("GIT_CONFIG_NOSYSTEM", "1")
                            .env("LC_ALL", "C")
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());
                        let mut child = c.spawn().unwrap();
                        let mut pipe = child.stdin.take().unwrap();
                        pipe.write_all(stdin.unwrap_or_default()).unwrap();
                        drop(pipe);
                        let out = child.wait_with_output().unwrap();
                        assert!(
                            out.status.success(),
                            "{} {args:?}: {}",
                            exe.display(),
                            String::from_utf8_lossy(&out.stderr)
                        );
                        out.stdout
                    };
                    git(&["init", "-q"], None);
                    git(&["config", "core.autocrlf", "false"], None);
                    if !eol.is_empty() {
                        git(&["config", "core.eol", eol], None);
                    }
                    let names: Vec<String> =
                        CONTENTS.iter().map(|(n, _)| format!("f-{n}")).collect();
                    for ((_, bytes), name) in CONTENTS.iter().zip(&names) {
                        std::fs::write(root.join(name), bytes).unwrap();
                    }
                    // the blobs are the exact bytes, whatever the attributes would make of them
                    let mut hash = vec!["hash-object", "-w", "--no-filters", "--"];
                    hash.extend(names.iter().map(String::as_str));
                    let oids = String::from_utf8(git(&hash, None)).unwrap();
                    let mut info = String::new();
                    for (oid, name) in oids.lines().zip(&names) {
                        writeln!(info, "100644 {oid}\t{name}").unwrap();
                    }
                    git(
                        &["update-index", "--add", "--index-info"],
                        Some(info.as_bytes()),
                    );
                    if !attrs.is_empty() {
                        std::fs::write(root.join(".gitattributes"), format!("f* {attrs}\n"))
                            .unwrap();
                        git(&["add", ".gitattributes"], None);
                    }
                    for name in &names {
                        std::fs::write(root.join(name), b"dirty\r\nworktree\n").unwrap();
                    }
                    let mut checkout = vec!["-c", "submodule.recurse=0", "checkout", "--"];
                    checkout.extend(names.iter().map(String::as_str));
                    git(&checkout, None);
                    let what = format!("{} attrs {attrs:?} core.eol {eol:?}", exe.display());
                    let raw = CONTENTS.iter().zip(&names).all(|((_, bytes), name)| {
                        std::fs::read(root.join(name)).unwrap() == *bytes
                    });
                    let diff = git(
                        &[
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--ignore-submodules",
                        ],
                        None,
                    );
                    if allowed {
                        assert!(raw, "{what}: the checkout converted a blob");
                        assert!(
                            diff.is_empty(),
                            "{what}: {}",
                            String::from_utf8_lossy(&diff)
                        );
                        checked += 1;
                    } else {
                        assert_eq!(
                            attrs, "text eol=crlf",
                            "{what}: only the control line is refused"
                        );
                        assert!(!raw, "{what}: the refused line must convert something");
                    }
                }
            }
        }
        assert!(checked >= 13 * gits.len(), "{checked}");
    }

    /// The input rules: missing is plain; a symlink, a directory, an error or a file of 100 MB or more refuses.
    #[cfg(unix)]
    #[test]
    fn inputs_must_be_missing_or_plain_regular_files() {
        let d = tempfile::tempdir().unwrap();
        let at = |rel: &str| d.path().join(rel);
        std::fs::write(at("plain"), "*.png binary\n").unwrap();
        std::fs::write(at("text"), "* text\n").unwrap();
        std::fs::create_dir_all(at("dir")).unwrap();
        std::os::unix::fs::symlink("plain", at("link")).unwrap();
        let f = std::fs::File::create(at("huge")).unwrap();
        f.set_len(MAX_FILE_SIZE).unwrap();
        let check = |rel: &str| input_plain(&at(rel), St::lstat_and_stat(&at(rel)), false);
        assert!(check("missing"));
        assert!(check("plain"));
        assert!(!check("text"));
        assert!(!check("dir"));
        assert!(!check("link"), "a symlinked attributes file");
        assert!(!check("huge"), "100 MB or more");
        let err = St {
            kind: Kind::Error(13),
            ..St::MISSING
        };
        assert!(!input_plain(&at("plain"), (err, err), false), "unreadable");
    }
}
