//! SHA-1 (FIPS 180-4), for checking that a loose object's bytes hash to its name as git's `parse_object` does
//! (ADR-153). No new crate: the function is small, has no secret inputs, and is checked against the standard vectors
//! and against git's own object names. git itself uses SHA-1DC, which equals SHA-1 on every input except a
//! collision-attack block, which git refuses (an object git could not have written).
//!
//! Owner: WP-18.

/// The initial hash value.
const H0: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];

/// SHA-1 of the concatenation of `parts`.
pub(crate) fn digest(parts: &[&[u8]]) -> [u8; 20] {
    let mut st = State {
        h: H0,
        block: [0; 64],
        fill: 0,
    };
    let mut total: u64 = 0;
    for part in parts {
        total = total.wrapping_add(u64::try_from(part.len()).unwrap_or(u64::MAX));
        st.feed(part);
    }
    // the padding: 0x80, zeros up to 56 bytes into a block, then the bit length (big endian)
    let zeros = 119_usize.saturating_sub(st.fill) % 64;
    st.feed(&[0x80]);
    st.feed([0_u8; 64].get(..zeros).unwrap_or_default());
    st.feed(&total.wrapping_mul(8).to_be_bytes());
    let mut out = [0_u8; 20];
    for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(st.h) {
        *chunk = word.to_be_bytes();
    }
    out
}

/// The running hash and the partial block.
struct State {
    h: [u32; 5],
    block: [u8; 64],
    /// Bytes in `block` (always below 64 between calls).
    fill: usize,
}

impl State {
    fn feed(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let free = self.block.get_mut(self.fill..).unwrap_or_default();
            let n = free.len().min(bytes.len());
            let (now, later) = bytes.split_at_checked(n).unwrap_or((bytes, &[]));
            for (dst, src) in free.iter_mut().zip(now) {
                *dst = *src;
            }
            self.fill = self.fill.saturating_add(n);
            bytes = later;
            if self.fill >= 64 {
                compress(&mut self.h, &self.block);
                self.fill = 0;
            }
        }
    }
}

/// One 64-byte block.
#[expect(
    clippy::many_single_char_names,
    reason = "the standard's names (FIPS 180-4 §6.1.2)"
)]
fn compress(h: &mut [u32; 5], block: &[u8; 64]) {
    let mut w = [0_u32; 80];
    for (slot, c) in w.iter_mut().zip(block.as_chunks::<4>().0) {
        *slot = u32::from_be_bytes(*c);
    }
    for t in 16_usize..80 {
        let v = w.get(t.wrapping_sub(3)).copied().unwrap_or(0)
            ^ w.get(t.wrapping_sub(8)).copied().unwrap_or(0)
            ^ w.get(t.wrapping_sub(14)).copied().unwrap_or(0)
            ^ w.get(t.wrapping_sub(16)).copied().unwrap_or(0);
        if let Some(slot) = w.get_mut(t) {
            *slot = v.rotate_left(1);
        }
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (t, wt) in w.iter().enumerate() {
        let (f, k) = match t {
            0..20 => ((b & c) | (!b & d), 0x5A82_7999),
            20..40 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6_u32),
        };
        let tmp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*wt);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = tmp;
    }
    for (x, v) in h.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        crate::odb::hex(b)
    }

    #[test]
    fn standard_vectors() {
        assert_eq!(
            hex(&digest(&[])),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex(&digest(&[b"abc"])),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&digest(&[
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            ])),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&digest(&[&million])),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    /// git's object names: the empty blob and the empty tree.
    #[test]
    fn git_object_names() {
        assert_eq!(
            hex(&digest(&[b"blob 0\0"])),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        assert_eq!(
            hex(&digest(&[b"tree 0\0"])),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        );
    }

    /// Every split of the input into parts, and every length around the block and padding boundaries.
    #[test]
    fn parts_and_boundaries() {
        let data: Vec<u8> = (0..200_u8).collect();
        for n in [0, 1, 55, 56, 57, 63, 64, 65, 119, 120, 128, 200] {
            let whole = digest(&[&data[..n]]);
            for cut in 0..=n {
                assert_eq!(digest(&[&data[..cut], &data[cut..n]]), whole, "{n} {cut}");
            }
        }
        // 55 and 56 bytes straddle the one-block / two-block padding
        assert_eq!(
            hex(&digest(&[&[b'a'; 55]])),
            "c1c8bbdc22796e28c0e15163d20899b65621d65a"
        );
        assert_eq!(
            hex(&digest(&[&[b'a'; 56]])),
            "c2db330f6083854c99d4b5bfb6e8f29f201be699"
        );
    }
}
