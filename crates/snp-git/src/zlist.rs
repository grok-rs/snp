//! NUL-separated byte buffer + spans: the `-z` output of git kept as one allocation (02 G.0 #8, 02 G.10).
//!
//! Owner: WP-18.

use std::fmt;

/// NUL-separated byte buffer + spans, git order preserved, never sorted (02 G.0 #8).
///
/// Parsing reproduces `git.zsplit` (`git.py:19-24`): `s.strip('\0')`, then `split('\0')` — leading and trailing NULs
/// are dropped, interior empty records are kept (they never occur in git's own output). Paths are raw bytes, never
/// decoded (D-CRASH-12). `spans` are half-open `(start, end)` byte offsets into `buf`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ZList {
    buf: Vec<u8>,
    spans: Vec<(u32, u32)>,
}

impl fmt::Debug for ZList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.iter().map(String::from_utf8_lossy))
            .finish()
    }
}

/// A git output too large to index with `u32` offsets (4 GiB): never produced by a real repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLarge;

fn span_of(buf: &[u8], s: u32, e: u32) -> Option<&[u8]> {
    buf.get(usize::try_from(s).ok()?..usize::try_from(e).ok()?)
}

impl ZList {
    /// Number of entries.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// No entries.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Entry `i` (raw bytes, as git printed it).
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        let &(s, e) = self.spans.get(i)?;
        span_of(&self.buf, s, e)
    }

    /// Entries in git order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.spans
            .iter()
            .filter_map(|&(s, e)| span_of(&self.buf, s, e))
    }

    /// `(buf, spans)`: the single buffer and the half-open `(start, end)` offsets of every entry into it.
    pub fn into_parts(self) -> (Vec<u8>, Vec<(u32, u32)>) {
        (self.buf, self.spans)
    }

    /// `git.zsplit(buf)` without copying: the spans index `buf` directly.
    pub(crate) fn from_z(buf: Vec<u8>) -> Result<Self, TooLarge> {
        Self::split(buf, false)
    }

    /// `ls-files -z` + adjacent dedup: a record equal to the previous record is skipped, which is exactly what
    /// `ls-files --deduplicate` prints (`builtin/ls-files.c`; D-CRASH-26, 02 G.10).
    pub(crate) fn from_z_dedup(buf: Vec<u8>) -> Result<Self, TooLarge> {
        Self::split(buf, true)
    }

    fn split(buf: Vec<u8>, dedup: bool) -> Result<Self, TooLarge> {
        let total = u32::try_from(buf.len()).map_err(|_| TooLarge)?;
        let mut spans: Vec<(u32, u32)> = Vec::new();
        // s.strip('\0')
        let Some(first) = buf.iter().position(|&b| b != 0) else {
            return Ok(Self {
                buf,
                spans: Vec::new(),
            });
        };
        let last = buf
            .iter()
            .rposition(|&b| b != 0)
            .map_or(buf.len(), |i| i.saturating_add(1));
        let mut start = u32::try_from(first).map_err(|_| TooLarge)?;
        let end = u32::try_from(last).map_err(|_| TooLarge)?.min(total);
        let body = buf.get(first..last).unwrap_or_default();
        let push = |s: u32, e: u32, spans: &mut Vec<(u32, u32)>| {
            if dedup
                && let Some(&(ps, pe)) = spans.last()
                && span_of(&buf, ps, pe) == span_of(&buf, s, e)
            {
                return;
            }
            spans.push((s, e));
        };
        for nul in memchr::memchr_iter(0, body) {
            let at = u32::try_from(first.saturating_add(nul)).map_err(|_| TooLarge)?;
            push(start, at, &mut spans);
            start = at.saturating_add(1);
        }
        push(start, end, &mut spans);
        Ok(Self { buf, spans })
    }

    /// A list built from records (copied into one buffer), in the given order.
    #[cfg(test)]
    pub(crate) fn from_records<'a>(
        records: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Self, TooLarge> {
        let mut out = Self::default();
        for r in records {
            out.push(r)?;
        }
        Ok(out)
    }

    /// Appends one record.
    pub(crate) fn push(&mut self, record: &[u8]) -> Result<(), TooLarge> {
        let s = u32::try_from(self.buf.len()).map_err(|_| TooLarge)?;
        let len = u32::try_from(record.len()).map_err(|_| TooLarge)?;
        let e = s.checked_add(len).ok_or(TooLarge)?;
        self.buf.extend_from_slice(record);
        self.spans.push((s, e));
        Ok(())
    }

    /// Whether `record` is already an entry (linear scan; used for small unions only).
    pub(crate) fn contains(&self, record: &[u8]) -> bool {
        self.iter().any(|r| r == record)
    }

    /// A list over `buf` with explicit spans (index-entry parsing keeps git's buffer and points at the path part of
    /// each record); spans outside `buf` are dropped.
    pub(crate) fn from_parts(buf: Vec<u8>, spans: Vec<(u32, u32)>) -> Self {
        let spans = spans
            .into_iter()
            .filter(|&(s, e)| span_of(&buf, s, e).is_some())
            .collect();
        Self { buf, spans }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z(b: &[u8]) -> Vec<Vec<u8>> {
        ZList::from_z(b.to_vec())
            .unwrap()
            .iter()
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn py(b: &[u8]) -> Vec<Vec<u8>> {
        snp_pycompat::precommit::zsplit(b)
            .map(<[u8]>::to_vec)
            .collect()
    }

    #[test]
    fn zsplit_vectors() {
        // git_test.py:201-211 (T10-GIT-08 is ported in tests/catalog.rs as well).
        assert_eq!(z(b"foo\0bar\0"), vec![b"foo".to_vec(), b"bar".to_vec()]);
        assert_eq!(z(b"foo\0"), vec![b"foo".to_vec()]);
        assert_eq!(z(b""), Vec::<Vec<u8>>::new());
        assert_eq!(z(b"foo"), vec![b"foo".to_vec()]);
    }

    #[test]
    fn matches_pycompat_on_edge_cases() {
        for case in [
            &b"\0\0a\0\0b\0\0"[..],
            b"\0",
            b"\0\0\0",
            b"a\0\0",
            b"\0a",
            b"a\0b\0c",
            b"\xff\xfe\0\x80",
        ] {
            assert_eq!(z(case), py(case), "{case:?}");
        }
    }

    #[test]
    fn dedup_drops_adjacent_duplicates_only() {
        let l = ZList::from_z_dedup(b"a\0a\0a\0b\0a\0".to_vec()).unwrap();
        let got: Vec<&[u8]> = l.iter().collect();
        assert_eq!(got, vec![&b"a"[..], b"b", b"a"]);
    }

    #[test]
    fn accessors_and_parts() {
        let l = ZList::from_z(b"\0x\0yy\0".to_vec()).unwrap();
        assert_eq!(l.len(), 2);
        assert!(!l.is_empty());
        assert_eq!(l.get(0), Some(&b"x"[..]));
        assert_eq!(l.get(1), Some(&b"yy"[..]));
        assert_eq!(l.get(2), None);
        let (buf, spans) = l.into_parts();
        assert_eq!(buf, b"\0x\0yy\0");
        assert_eq!(spans, vec![(1, 2), (3, 5)]);
    }

    #[test]
    fn records_and_contains() {
        let mut l = ZList::from_records([&b"a"[..], b"b c"]).unwrap();
        l.push(b"").unwrap();
        assert!(l.contains(b"b c"));
        assert!(!l.contains(b"z"));
        assert_eq!(format!("{l:?}"), r#"["a", "b c", ""]"#);
        assert!(ZList::default().is_empty());
    }
}
