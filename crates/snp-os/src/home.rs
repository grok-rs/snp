//! `os.path.expanduser` over the captured environment, and the default cache root.
//!
//! Owner: WP-15.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use snp_core::env::Env;
use snp_pycompat::ospath::{self, HomeLookup};

/// Python semantics incl. `HOME=""` and Windows `USERPROFILE` (15 §A4.9-10).
///
/// `os.path.expanduser(p)` (`posixpath.py`/`ntpath.py` of CPython 3.12) evaluated against `env` instead of
/// `os.environ`. POSIX: `~` is `HOME` whenever the key exists (even empty: `HOME=""` gives `/`), else the passwd
/// entry of the real uid; `~user` is that user's passwd entry; an unknown user leaves the path unchanged. The passwd
/// database is read from `/etc/passwd` (pre-commit's `pwd` module also consults NSS sources such as LDAP; those
/// users fall back to "unchanged"). Windows: `USERPROFILE`, else `HOMEDRIVE` + `HOMEPATH`; `HOME` is ignored.
pub fn expanduser(p: &OsStr, env: &Env) -> OsString {
    let lookup_env = |k: &str| env.get(k).map(OsStr::to_os_string);
    let pw_current = passwd::current_home;
    let pw_named = passwd::named_home;
    let home = HomeLookup {
        env: &lookup_env,
        pw_current: &pw_current,
        pw_named: &pw_named,
    };
    ospath::expanduser(p, &home).into_owned()
}

/// `XDG_CACHE_HOME` (non-empty) else `expanduser("~/.cache")`.
///
/// The cache-root half of `store._get_default_directory` (`store.py:25-36`): `os.environ.get('XDG_CACHE_HOME') or
/// os.path.expanduser('~/.cache')`. `None` when `~` cannot be expanded (no `HOME`/`USERPROFILE` and no passwd
/// entry), where pre-commit would use the literal relative `~/.cache` directory.
pub fn default_cache_root(env: &Env) -> Option<PathBuf> {
    if let Some(xdg) = env.get_nonempty("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg));
    }
    let tilde = OsStr::new("~/.cache");
    let expanded = expanduser(tilde, env);
    if expanded == tilde {
        None
    } else {
        Some(PathBuf::from(expanded))
    }
}

/// The parts of `pwd` that `expanduser` needs, read from `/etc/passwd`.
mod passwd {
    use std::ffi::{OsStr, OsString};

    /// `pwd.getpwuid(os.getuid()).pw_dir`.
    #[cfg(unix)]
    pub(super) fn current_home() -> Option<OsString> {
        let uid = rustix::process::getuid().as_raw().to_string();
        lookup(|_, entry_uid| entry_uid == uid.as_bytes())
    }

    /// `pwd.getpwnam(name).pw_dir`.
    #[cfg(unix)]
    pub(super) fn named_home(name: &OsStr) -> Option<OsString> {
        use std::os::unix::ffi::OsStrExt;
        let name = name.as_bytes();
        if name.is_empty() || name.contains(&0) {
            return None;
        }
        lookup(|entry_name, _| entry_name == name)
    }

    /// The home directory of the first `/etc/passwd` entry accepted by `matches(name, uid)`.
    #[cfg(unix)]
    fn lookup(matches: impl Fn(&[u8], &[u8]) -> bool) -> Option<OsString> {
        use std::os::unix::ffi::OsStringExt;
        let data = crate::fs::read_capped(std::path::Path::new("/etc/passwd"), 16 << 20).ok()?;
        parse_home(&data, matches).map(|h| OsString::from_vec(h.to_vec()))
    }

    /// `name:passwd:uid:gid:gecos:dir:shell` lines; NIS `+`/`-` lines and malformed lines are skipped.
    #[cfg(any(unix, test))]
    pub(super) fn parse_home(data: &[u8], matches: impl Fn(&[u8], &[u8]) -> bool) -> Option<&[u8]> {
        data.split(|b| *b == b'\n').find_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.starts_with(b"+") || line.starts_with(b"-") || line.starts_with(b"#") {
                return None;
            }
            let mut fields = line.split(|b| *b == b':');
            let name = fields.next()?;
            let _pw = fields.next()?;
            let uid = fields.next()?;
            let _gid = fields.next()?;
            let _gecos = fields.next()?;
            let dir = fields.next()?;
            matches(name, uid).then_some(dir)
        })
    }

    /// Windows `expanduser` never consults a passwd database.
    #[cfg(not(unix))]
    pub(super) fn current_home() -> Option<OsString> {
        None
    }

    /// Windows `expanduser` never consults a passwd database.
    #[cfg(not(unix))]
    pub(super) fn named_home(_name: &OsStr) -> Option<OsString> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(p: &str, pairs: &[(&str, &str)]) -> String {
        expanduser(OsStr::new(p), &Env::from_pairs(pairs.iter().copied()))
            .into_string()
            .unwrap()
    }

    /// V-PE: `posixpath.expanduser` by `HOME` value (15 §A4.9).
    #[cfg(unix)]
    #[test]
    fn v_pe_posix_expanduser_vectors() {
        let inputs = ["~", "~/", "~/.cache", "x/~", "~~", "~/~"];
        let table: [(&str, [&str; 6]); 6] = [
            (
                "/home/u",
                [
                    "/home/u",
                    "/home/u/",
                    "/home/u/.cache",
                    "x/~",
                    "~~",
                    "/home/u/~",
                ],
            ),
            (
                "/home/u/",
                [
                    "/home/u",
                    "/home/u/",
                    "/home/u/.cache",
                    "x/~",
                    "~~",
                    "/home/u/~",
                ],
            ),
            ("", ["/", "/", "/.cache", "x/~", "~~", "/~"]),
            ("/", ["/", "/", "/.cache", "x/~", "~~", "/~"]),
            ("//", ["/", "/", "/.cache", "x/~", "~~", "/~"]),
            (
                "rel/home",
                [
                    "rel/home",
                    "rel/home/",
                    "rel/home/.cache",
                    "x/~",
                    "~~",
                    "rel/home/~",
                ],
            ),
        ];
        for (home, expected) in table {
            for (input, want) in inputs.iter().zip(expected) {
                assert_eq!(exp(input, &[("HOME", home)]), want, "HOME={home:?} {input}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn unset_home_uses_passwd_and_unknown_users_stay() {
        // HOME unset: the passwd entry of the real uid (present on every CI image), else unchanged.
        let got = exp("~/x", &[]);
        assert!(got.ends_with("/x"), "{got}");
        assert_eq!(exp("~no-such-user-snp/x", &[]), "~no-such-user-snp/x");
        // `root` exists on every Unix and has an absolute home.
        assert!(exp("~root", &[("HOME", "/h")]).starts_with('/'));
    }

    #[test]
    fn passwd_parsing() {
        let data = b"# comment\n+nis::0:0:::\nroot:x:0:0:root:/root:/bin/sh\nu:x:1000:1000:U,,,:/home/u:/bin/bash\r\nbad line\nu:x:1001:1:dup:/home/dup:/bin/sh\n";
        let by_uid = |uid: &'static [u8]| passwd::parse_home(data, move |_, u| u == uid);
        assert_eq!(by_uid(b"0"), Some(&b"/root"[..]));
        assert_eq!(by_uid(b"1000"), Some(&b"/home/u"[..]));
        assert_eq!(by_uid(b"7"), None);
        // First match wins (getpwnam).
        assert_eq!(
            passwd::parse_home(data, |n, _| n == b"u"),
            Some(&b"/home/u"[..])
        );
    }

    proptest::proptest! {
        /// The passwd parser accepts arbitrary bytes without panicking and only returns a field of an input line.
        #[test]
        fn passwd_parsing_is_total(data in proptest::collection::vec(proptest::num::u8::ANY, 0..400)) {
            if let Some(dir) = passwd::parse_home(&data, |_, _| true) {
                proptest::prop_assert!(dir.len() <= data.len());
                proptest::prop_assert!(!dir.contains(&b'\n') && !dir.contains(&b':'));
            }
        }
    }

    #[test]
    fn default_cache_root_rules() {
        let root =
            |pairs: &[(&str, &str)]| default_cache_root(&Env::from_pairs(pairs.iter().copied()));
        assert_eq!(
            root(&[("XDG_CACHE_HOME", "/xdg"), ("HOME", "/h")]),
            Some(PathBuf::from("/xdg"))
        );
        if cfg!(unix) {
            // An empty XDG_CACHE_HOME counts as unset.
            assert_eq!(
                root(&[("XDG_CACHE_HOME", ""), ("HOME", "/h")]),
                Some(PathBuf::from("/h/.cache"))
            );
            assert_eq!(root(&[("HOME", "")]), Some(PathBuf::from("/.cache")));
        } else {
            assert_eq!(
                root(&[("USERPROFILE", r"C:\Users\u")]),
                Some(PathBuf::from(r"C:\Users\u/.cache"))
            );
            assert_eq!(root(&[]), None);
        }
    }

    /// V-NE: `ntpath.expanduser` by environment (15 §A4.10), through the code path Windows uses.
    #[test]
    fn v_ne_nt_expanduser_vectors() {
        type Row<'a> = (&'a [(&'a str, &'a str)], [&'a str; 4]);
        let inputs = ["~", "~/.cache", "~\\.cache", "~other\\x"];
        let rows: [Row<'_>; 7] = [
            (
                &[("USERPROFILE", "C:\\Users\\u")],
                [
                    "C:\\Users\\u",
                    "C:\\Users\\u/.cache",
                    "C:\\Users\\u\\.cache",
                    "~other\\x",
                ],
            ),
            (
                &[("USERPROFILE", "")],
                ["", "/.cache", "\\.cache", "~other\\x"],
            ),
            (
                &[("HOMEDRIVE", "D:"), ("HOMEPATH", "\\home\\u")],
                [
                    "D:\\home\\u",
                    "D:\\home\\u/.cache",
                    "D:\\home\\u\\.cache",
                    "~other\\x",
                ],
            ),
            (
                &[("HOMEPATH", "\\home\\u")],
                [
                    "\\home\\u",
                    "\\home\\u/.cache",
                    "\\home\\u\\.cache",
                    "~other\\x",
                ],
            ),
            (&[], ["~", "~/.cache", "~\\.cache", "~other\\x"]),
            (
                &[("HOME", "/home/posix")],
                ["~", "~/.cache", "~\\.cache", "~other\\x"],
            ),
            (
                &[("USERNAME", "u"), ("USERPROFILE", "C:\\Users\\u")],
                [
                    "C:\\Users\\u",
                    "C:\\Users\\u/.cache",
                    "C:\\Users\\u\\.cache",
                    "C:\\Users\\other\\x",
                ],
            ),
        ];
        for (env, expected) in rows {
            for (input, want) in inputs.iter().zip(expected) {
                let lookup = |k: &str| {
                    env.iter()
                        .find(|(n, _)| *n == k)
                        .map(|(_, v)| v.as_bytes().to_vec())
                };
                let got = snp_pycompat::ntpath::expanduser(input.as_bytes(), &lookup);
                assert_eq!(&*got, want.as_bytes(), "{env:?} {input}");
                if cfg!(windows) {
                    assert_eq!(exp(input, env), *want, "{env:?} {input}");
                }
            }
        }
    }
}
