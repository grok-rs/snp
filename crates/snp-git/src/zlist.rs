//! NUL-separated byte buffer + spans: the `-z` output of git kept as one allocation (02 G.0 #8, 02 G.10).
//!
//! Owner: WP-18.

use std::fmt;

use split::FindByte;

mod split;

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

/// The production [`FindByte`]: `memchr::memchr`. Its contract is a trusted item of `proofs/registry.toml`
/// (ADR-095 §10 row 8), tested against the verified [`split::Scalar`] by `memchr_matches_scalar`.
#[derive(Debug, Clone, Copy)]
struct Memchr;

impl FindByte for Memchr {
    fn find_byte(&self, needle: u8, hay: &[u8], from: usize) -> Option<usize> {
        let i = memchr::memchr(needle, hay.get(from..)?)?;
        from.checked_add(i)
    }
}

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

    /// Unverified shell of kernel K5b (ADR-095, docs/VERIFICATION.md §9.2): the spans come from the verified
    /// [`split::split_spans`] with the memchr finder.
    #[expect(
        clippy::disallowed_macros,
        reason = "debug-only differential check of the trusted `Memchr: FindByte` (ADR-095 §10 row 8)"
    )]
    fn split(buf: Vec<u8>, dedup: bool) -> Result<Self, TooLarge> {
        let spans = split::split_spans(&Memchr, &buf, dedup).ok_or(TooLarge)?;
        // Every debug build (tests, fuzzing) checks the trusted memchr finder against the verified scalar one.
        debug_assert_eq!(
            split::split_spans(&split::Scalar, &buf, dedup).as_ref(),
            Some(&spans)
        );
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

    /// The [[trusted]] `Memchr: FindByte` of `proofs/registry.toml` (ADR-095 §10 row 8): memchr agrees with the
    /// verified scalar finder on every haystack over `{0, 1, 2}` up to length 8 (every needle, every `from`,
    /// including past the end) and on seeded random haystacks long enough for memchr's vectorised paths.
    #[test]
    fn memchr_matches_scalar() {
        fn check(hay: &[u8]) {
            for needle in 0..4u8 {
                for from in 0..=hay.len() + 2 {
                    assert_eq!(
                        Memchr.find_byte(needle, hay, from),
                        split::Scalar.find_byte(needle, hay, from),
                        "{hay:?} {needle} {from}"
                    );
                }
            }
            assert_eq!(Memchr.find_byte(0, hay, usize::MAX), None);
        }
        let mut hay = Vec::new();
        for len in 0..=8u32 {
            for code in 0..3u32.checked_pow(len).unwrap() {
                hay.clear();
                let mut c = code;
                for _ in 0..len {
                    hay.push(u8::try_from(c % 3).unwrap());
                    c /= 3;
                }
                check(&hay);
            }
        }
        // SplitMix64 (seed fixed), lengths up to 300 with sparse needles so the hits land past the SIMD prologue.
        let mut state: u64 = 0x5eed_0005_0000_0001;
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        for _ in 0..2_000 {
            let len = usize::try_from(next() % 300).unwrap();
            let hay: Vec<u8> = (0..len)
                .map(|_| {
                    if next() % 61 == 0 {
                        u8::try_from(next() % 4).unwrap()
                    } else {
                        7
                    }
                })
                .collect();
            check(&hay);
        }
    }

    /// The verified spans equal `git.zsplit` (pycompat) and its adjacent dedup on every buffer over `{0, a, b}` up
    /// to length 8.
    #[test]
    fn split_matches_pycompat_exhaustively() {
        let mut buf = Vec::new();
        for len in 0..=8u32 {
            for code in 0..3u32.checked_pow(len).unwrap() {
                buf.clear();
                let mut c = code;
                for _ in 0..len {
                    buf.push([0, b'a', b'b'][usize::try_from(c % 3).unwrap()]);
                    c /= 3;
                }
                let mut want = py(&buf);
                assert_eq!(z(&buf), want, "{buf:?}");
                want.dedup();
                let got: Vec<Vec<u8>> = ZList::from_z_dedup(buf.clone())
                    .unwrap()
                    .iter()
                    .map(<[u8]>::to_vec)
                    .collect();
                assert_eq!(got, want, "{buf:?}");
            }
        }
    }

    #[test]
    fn records_and_push() {
        let mut l = ZList::from_records([&b"a"[..], b"b c"]).unwrap();
        l.push(b"").unwrap();
        assert_eq!(l.get(1), Some(&b"b c"[..]));
        assert_eq!(format!("{l:?}"), r#"["a", "b c", ""]"#);
        assert!(ZList::default().is_empty());
    }
}
