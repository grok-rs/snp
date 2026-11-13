//! Entry-name and symlink-target validation (ADR-077).
//!
//! Owner: WP-16.

use std::ffi::OsString;

use super::entry::entry_components;
use crate::NetError;

/// A validated path inside the extracted tree: one or more plain components (never empty, `.`, `..`, absolute or
/// drive-prefixed). The empty vector is the tree root (only produced by symlink resolution).
pub(crate) type RelPath = Vec<OsString>;

/// The error for an entry named `raw`.
pub(crate) fn unsafe_entry(raw: &[u8]) -> NetError {
    NetError::UnsafeEntry(String::from_utf8_lossy(raw).into_owned())
}

/// `raw` starts like an absolute or drive-prefixed path on any OS: `/x`, `\x`, `C:…`.
fn is_rooted(raw: &[u8]) -> bool {
    matches!(raw.first(), Some(b'/' | b'\\'))
        || matches!(raw, [d, b':', ..] if d.is_ascii_alphabetic())
}

/// Splits a name on `/` (and `\` on Windows, where both are separators).
fn split(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split(|b| *b == b'/' || (cfg!(windows) && *b == b'\\'))
}

/// Windows device names that open a device instead of a file whatever the directory (`CON`, `NUL.txt`, …).
#[cfg(any(not(unix), test))]
fn is_windows_reserved(comp: &str) -> bool {
    let stem = comp.split('.').next().unwrap_or(comp).trim_end();
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper
                .as_bytes()
                .get(3)
                .is_some_and(|c| (b'1'..=b'9').contains(c)))
}

/// One plain component (not empty, `.` or `..`) as an OS string, or `None` when it cannot be represented safely
/// on this OS.
fn component(comp: &[u8]) -> Option<OsString> {
    if comp.is_empty() || comp == b"." || comp == b".." || comp.contains(&0) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(std::ffi::OsStr::from_bytes(comp).to_os_string())
    }
    #[cfg(not(unix))]
    {
        let s = std::str::from_utf8(comp).ok()?;
        let bad = s.contains([':', '\\', '<', '>', '"', '|', '?', '*'])
            || s.ends_with(['.', ' '])
            || s.chars().any(char::is_control)
            || is_windows_reserved(s);
        if bad {
            return None;
        }
        Some(OsString::from(s))
    }
}

/// The validated path of an archive entry named `raw`, with the first `strip` components removed (`tar
/// --strip-components`). `Ok(None)`: the entry lies entirely within the stripped prefix (e.g. the top directory).
/// Absolute, drive-prefixed and `..` names are rejected; empty and `.` components are dropped.
///
/// The name check is the verified kernel [`entry_components`] (ADR-095); this shell only converts each component
/// to an OS string, which on Windows adds refusals (non-UTF-8, NTFS-illegal characters, reserved device names) and
/// never removes one. Every component is converted before stripping, so the stripped prefix is checked too.
pub(crate) fn entry_path(raw: &[u8], strip: u8) -> Result<Option<RelPath>, NetError> {
    let spans = entry_components(raw, cfg!(windows)).map_err(|_| unsafe_entry(raw))?;
    let mut comps: RelPath = Vec::new();
    for (start, end) in spans {
        let c = raw.get(start..end).and_then(component);
        comps.push(c.ok_or_else(|| unsafe_entry(raw))?);
    }
    let strip = usize::from(strip);
    if comps.len() <= strip {
        return Ok(None);
    }
    Ok(Some(
        comps.get(strip..).map_or_default(<[OsString]>::to_vec),
    ))
}

/// Resolves the target of the symlink `link` lexically against the extracted tree: the result stays inside the tree
/// and no component on the way (including the final one) is a symlink of the archive (`is_symlink`, the set of every
/// symlink entry), so chains such as `x -> .` + `y -> x/..` are refused. Absolute and drive-prefixed targets are
/// refused. The resolved path (empty = the tree root) is returned.
///
/// Name equality alone cannot see every symlink: on a case-insensitive or normalising filesystem (macOS, Windows)
/// `x` opens the archive's `X`. So every component a `..` steps back over must also be a real directory of the tree
/// right now (`is_real_dir`: opened without following symlinks). Pass-1 directories never become symlinks later
/// (creating a link over an existing name fails, case-insensitively too), a name that does not exist yet is refused,
/// and without `..` a lexical path through symlinks that each stay inside the tree physically stays inside too.
pub(crate) fn resolve_symlink(
    link: &[OsString],
    target: &[u8],
    is_symlink: &dyn Fn(&[OsString]) -> bool,
    is_real_dir: &mut dyn FnMut(&[OsString]) -> bool,
) -> Option<RelPath> {
    if target.is_empty() || is_rooted(target) {
        return None;
    }
    let (_, parent) = link.split_last()?;
    // The link must not itself live below another symlink.
    for i in 1..=parent.len() {
        if is_symlink(parent.get(..i)?) {
            return None;
        }
    }
    let mut stack: RelPath = parent.to_vec();
    for c in split(target) {
        match c {
            b"" | b"." => {}
            b".." => {
                if stack.is_empty() || !is_real_dir(&stack) {
                    return None;
                }
                stack.pop()?;
            }
            _ => {
                stack.push(component(c)?);
                if is_symlink(&stack) {
                    return None;
                }
            }
        }
    }
    Some(stack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str, strip: u8) -> Result<Option<Vec<String>>, ()> {
        entry_path(s.as_bytes(), strip)
            .map(|o| o.map(|v| v.iter().map(|c| c.to_string_lossy().into_owned()).collect()))
            .map_err(|_| ())
    }

    #[test]
    fn entry_names() {
        assert_eq!(
            p("a/b/c", 0),
            Ok(Some(vec!["a".into(), "b".into(), "c".into()]))
        );
        assert_eq!(p("./a//b/", 0), Ok(Some(vec!["a".into(), "b".into()])));
        assert_eq!(p("top/", 1), Ok(None));
        assert_eq!(p("top/bin/x", 1), Ok(Some(vec!["bin".into(), "x".into()])));
        assert_eq!(p("top/bin/x", 3), Ok(None));
        for bad in [
            "",
            "/etc/passwd",
            "\\x",
            "C:/x",
            "c:x",
            "a/../b",
            "..",
            "a/..",
            "../x",
        ] {
            assert_eq!(p(bad, 0), Err(()), "{bad:?}");
        }
        // `..` is refused even inside the stripped prefix.
        assert_eq!(p("../top/x", 1), Err(()));
    }

    /// The name check as it was before the verified kernel (ADR-095), with the platform made a parameter: the
    /// components (not yet converted to OS strings), or `None` for a refused name.
    fn reference(raw: &[u8], windows: bool) -> Option<Vec<Vec<u8>>> {
        let rooted = matches!(raw.first(), Some(b'/' | b'\\'))
            || matches!(raw, [d, b':', ..] if d.is_ascii_alphabetic());
        if raw.is_empty() || rooted {
            return None;
        }
        let mut comps = Vec::new();
        for c in raw.split(|b| *b == b'/' || (windows && *b == b'\\')) {
            if c.is_empty() || c == b"." {
                continue;
            }
            if c == b".." || c.contains(&0) {
                return None;
            }
            comps.push(c.to_vec());
        }
        Some(comps)
    }

    /// The kernel equals the former check on every name over a separator-rich alphabet up to length 6, for both
    /// values of `windows` (the proof covers every input; this pins the extraction itself).
    #[test]
    fn kernel_matches_the_former_check() {
        const ALPHABET: &[u8] = b"a./\\\0:C";
        let mut name = Vec::new();
        let mut cases = 0_usize;
        for len in 0..=6_u32 {
            let total = ALPHABET.len().checked_pow(len).unwrap_or(0);
            for mut code in 0..total {
                name.clear();
                for _ in 0..len {
                    name.extend(ALPHABET.get(code % ALPHABET.len()));
                    code /= ALPHABET.len();
                }
                for windows in [false, true] {
                    let got = entry_components(&name, windows).ok().map(|spans| {
                        spans
                            .iter()
                            .map(|&(s, e)| name.get(s..e).map(<[u8]>::to_vec))
                            .collect::<Option<Vec<_>>>()
                    });
                    assert_eq!(
                        got,
                        reference(&name, windows).map(Some),
                        "{name:?} windows={windows}"
                    );
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 2 * (1 + 7 + 49 + 343 + 2401 + 16807 + 117_649));
    }

    #[test]
    fn kernel_separators_follow_the_platform_flag() {
        use super::super::entry::Refusal;
        assert!(entry_components(b"a\\..\\b", true).is_err());
        assert_eq!(entry_components(b"a\\..\\b", false), Ok(vec![(0, 6)]));
        assert_eq!(
            entry_components(b"a\\.\\\\b/c", true),
            Ok(vec![(0, 1), (5, 6), (7, 8)])
        );
        assert_eq!(entry_components(b"./x/", false), Ok(vec![(2, 3)]));
        assert_eq!(entry_components(b"", false), Err(Refusal::Rooted));
        assert_eq!(entry_components(b"c:x", false), Err(Refusal::Rooted));
        assert_eq!(entry_components(b"a/../b", false), Err(Refusal::DotDot));
        assert_eq!(entry_components(b"a/b\0/..", false), Err(Refusal::Nul));
        // A NUL or `..` in what --strip-components removes still refuses the whole name.
        assert!(entry_path(b"t\0p/x", 1).is_err());
        assert!(entry_path(b"a/../x/y", 2).is_err());
    }

    #[test]
    fn reserved_windows_names() {
        assert!(is_windows_reserved("CON"));
        assert!(is_windows_reserved("nul.txt"));
        assert!(is_windows_reserved("com1"));
        assert!(!is_windows_reserved("com10"));
        assert!(!is_windows_reserved("console"));
    }

    fn rel(s: &str) -> RelPath {
        s.split('/')
            .filter(|c| !c.is_empty())
            .map(OsString::from)
            .collect()
    }

    /// Every path that is not a symlink of `links` (nor below one) is a real directory.
    fn resolve(link: &str, target: &str, links: &[&str]) -> Option<String> {
        resolve_in(link, target, links, None)
    }

    /// `dirs`: the real directories of the tree (`None`: everything that is not a symlink).
    fn resolve_in(
        link: &str,
        target: &str,
        links: &[&str],
        dirs: Option<&[&str]>,
    ) -> Option<String> {
        let set: Vec<RelPath> = links.iter().map(|l| rel(l)).collect();
        let is_link = |p: &[OsString]| set.iter().any(|s| s == p);
        let mut is_dir = |p: &[OsString]| match dirs {
            Some(d) => d.iter().any(|d| rel(d) == p),
            None => !(1..=p.len()).any(|i| p.get(..i).is_some_and(is_link)),
        };
        resolve_symlink(&rel(link), target.as_bytes(), &is_link, &mut is_dir).map(|r| {
            r.iter()
                .map(|c| c.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
    }

    #[test]
    fn symlink_resolution() {
        assert_eq!(
            resolve("bin/npm", "../lib/npm/cli.js", &["bin/npm"]).as_deref(),
            Some("lib/npm/cli.js")
        );
        assert_eq!(resolve("x", ".", &["x"]).as_deref(), Some(""));
        assert_eq!(resolve("a/b", "c", &["a/b"]).as_deref(), Some("a/c"));
        // Escapes, absolute targets.
        assert_eq!(resolve("x", "..", &["x"]), None);
        assert_eq!(resolve("a/x", "../../etc", &["a/x"]), None);
        assert_eq!(resolve("x", "/etc/passwd", &["x"]), None);
        assert_eq!(resolve("x", "C:\\Windows", &["x"]), None);
        assert_eq!(resolve("x", "", &["x"]), None);
        // Chains: through another symlink, onto another symlink, onto itself, below a symlink.
        assert_eq!(resolve("y", "x/..", &["x", "y"]), None);
        assert_eq!(resolve("y", "x", &["x", "y"]), None);
        assert_eq!(resolve("x", "x", &["x"]), None);
        assert_eq!(resolve("x/y", "z", &["x", "x/y"]), None);
    }

    /// A case-insensitive filesystem: `x` names the symlink `X`, which is no real directory (ADR-077 vector smuggled
    /// through a case variant).
    #[test]
    fn dot_dot_needs_a_real_directory() {
        // `X -> .`, `y -> x/..`: `x` is not a directory of the tree (it is `X`, a symlink, or nothing at all).
        assert_eq!(resolve_in("y", "x/..", &["X", "y"], Some(&[])), None);
        // `d/x -> ..`, `y -> d/X/..`.
        assert_eq!(resolve_in("y", "d/X/..", &["d/x", "y"], Some(&["d"])), None);
        // A real directory may be stepped back over.
        assert_eq!(
            resolve_in(
                "bin/npm",
                "../lib/cli.js",
                &["bin/npm"],
                Some(&["bin", "lib"])
            )
            .as_deref(),
            Some("lib/cli.js")
        );
        assert_eq!(
            resolve_in("a/l", "../b/../c", &["a/l"], Some(&["a", "b"])).as_deref(),
            Some("c")
        );
        // ... but not one that does not exist (yet).
        assert_eq!(resolve_in("a/l", "../b/../c", &["a/l"], Some(&["a"])), None);
        assert_eq!(resolve_in("a/l", "../c", &["a/l"], Some(&[])), None);
    }
}
