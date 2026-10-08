//! CPython's `random.Random` (MT19937, `Modules/_randommodule.c`, `Lib/random.py`) as far as
//! pre-commit uses it: `Random(1542676187).shuffle(seq)` in `lang_base._shuffled`
//! (`lang_base.py:146-153`), which orders the file arguments before xargs partitions them — bit
//! exact (02 §11.7; verified for every n of the WP-11 acceptance list against CPython).

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

/// `lang_base.FIXED_RANDOM_SEED`.
const FIXED_RANDOM_SEED: u32 = 1_542_676_187;

/// The Mersenne Twister state of a CPython `random.Random` (`RandomObject`: 624 words + index).
#[derive(Clone, Debug)]
pub struct Mt19937 {
    mt: Box<[u32; N]>,
    index: usize,
}

impl Mt19937 {
    fn get(&self, i: usize) -> u32 {
        self.mt.get(i).copied().unwrap_or(0)
    }

    fn set(&mut self, i: usize, v: u32) {
        if let Some(slot) = self.mt.get_mut(i) {
            *slot = v;
        }
    }

    /// `init_genrand(s)`.
    fn init_genrand(s: u32) -> Self {
        let mut st = Self {
            mt: Box::new([0; N]),
            index: N,
        };
        st.set(0, s);
        for i in 1..N {
            let prev = st.get(i.saturating_sub(1));
            let idx = u32::try_from(i).unwrap_or(0);
            st.set(
                i,
                1_812_433_253_u32
                    .wrapping_mul(prev ^ (prev >> 30))
                    .wrapping_add(idx),
            );
        }
        st.index = N;
        st
    }

    /// `init_by_array(key)` (`_randommodule.c`) — what `random.seed(int)` (`random_seed`) does with
    /// the 32-bit words of `abs(seed)`, least significant first; an empty key is `[0]` (seed 0).
    #[must_use]
    pub fn init_by_array(key: &[u32]) -> Self {
        let zero = [0_u32];
        let key = if key.is_empty() { &zero[..] } else { key };
        let mut st = Self::init_genrand(19_650_218);
        let mut i = 1_usize;
        let mut j = 0_usize;
        for _ in 0..N.max(key.len()) {
            let prev = st.get(i.saturating_sub(1));
            let k = key.get(j).copied().unwrap_or(0);
            let jj = u32::try_from(j).unwrap_or(0);
            let v = (st.get(i) ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(k)
                .wrapping_add(jj);
            st.set(i, v);
            i = i.saturating_add(1);
            j = j.saturating_add(1);
            if i >= N {
                st.set(0, st.get(N - 1));
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = st.get(i.saturating_sub(1));
            let ii = u32::try_from(i).unwrap_or(0);
            let v =
                (st.get(i) ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(ii);
            st.set(i, v);
            i = i.saturating_add(1);
            if i >= N {
                st.set(0, st.get(N - 1));
                i = 1;
            }
        }
        st.set(0, 0x8000_0000);
        st
    }

    fn twist(&mut self) {
        let mag = |y: u32| if y & 1 == 0 { 0 } else { MATRIX_A };
        for kk in 0..N {
            let next = kk.wrapping_add(1) % N;
            let far = kk.wrapping_add(M) % N;
            let y = (self.get(kk) & UPPER_MASK) | (self.get(next) & LOWER_MASK);
            let v = self.get(far) ^ (y >> 1) ^ mag(y);
            self.set(kk, v);
        }
        self.index = 0;
    }

    /// `genrand_uint32()`: the next tempered 32-bit output (= `getrandbits(32)`).
    pub fn genrand_u32(&mut self) -> u32 {
        if self.index >= N {
            self.twist();
        }
        let mut y = self.get(self.index);
        self.index = self.index.saturating_add(1);
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `getrandbits(k)` for `k <= 64` (32-bit words filled from least to most significant).
    fn getrandbits(&mut self, k: u32) -> u64 {
        match k {
            0 => 0,
            1..=32 => u64::from(self.genrand_u32() >> 32_u32.saturating_sub(k)),
            _ => {
                let lo = u64::from(self.genrand_u32());
                let hi_bits = k.min(64).saturating_sub(32);
                let hi = u64::from(self.genrand_u32() >> 32_u32.saturating_sub(hi_bits));
                lo | (hi << 32)
            }
        }
    }

    /// `Random._randbelow_with_getrandbits(n)` (`random.py`): uniform in `[0, n)` by rejection
    /// sampling on `getrandbits(n.bit_length())`, whose words fill from the least significant end.
    /// Python never calls it with 0; here 0 returns 0 without consuming output.
    pub fn randbelow(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let k = n.bit_width();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }
}

/// `lang_base._shuffled(range(n))` (`lang_base.py:146-153`): `random.Random()` seeded with
/// `FIXED_RANDOM_SEED` (`version=1` is irrelevant for an int seed), then `Random.shuffle` (for `i`
/// from `n-1` down to 1, swap `i` with `randbelow(i + 1)`) — a fresh generator per call, so every
/// hook shuffles the same way. Apply the permutation to the file list: `files[out[k]]`. Lengths
/// beyond `u32::MAX + 1` (never reached) are truncated.
#[must_use]
pub fn precommit_shuffled_indices(n: usize) -> Vec<u32> {
    let mut v: Vec<u32> = (0..n).map_while(|i| u32::try_from(i).ok()).collect();
    let mut rng = Mt19937::init_by_array(&[FIXED_RANDOM_SEED]);
    for i in (1..v.len()).rev() {
        let bound = u64::try_from(i).unwrap_or(u64::MAX).saturating_add(1);
        let j = usize::try_from(rng.randbelow(bound)).unwrap_or(0);
        if let (Some(&a), Some(&b)) = (v.get(i), v.get(j)) {
            if let Some(x) = v.get_mut(i) {
                *x = b;
            }
            if let Some(x) = v.get_mut(j) {
                *x = a;
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn getrandbits_matches_cpython() {
        let mut r = Mt19937::init_by_array(&[FIXED_RANDOM_SEED]);
        let got: Vec<u32> = (0..5).map(|_| r.genrand_u32()).collect();
        assert_eq!(
            got,
            vec![
                2_472_326_011,
                2_061_721_674,
                2_625_518_588,
                881_996_946,
                1_343_846_257
            ]
        );
        let mut r = Mt19937::init_by_array(&[5489]);
        let got: Vec<u32> = (0..3).map(|_| r.genrand_u32()).collect();
        assert_eq!(got, vec![3_382_763_572, 956_215_839, 417_760_592]);
        let mut r = Mt19937::init_by_array(&[FIXED_RANDOM_SEED]);
        let got: Vec<u64> = (0..5).map(|_| r.randbelow(1000)).collect();
        assert_eq!(got, vec![589, 491, 625, 210, 320]);
        assert_eq!(r.getrandbits(40), 718_678_763_870);
        assert_eq!(r.randbelow((1 << 40) + 3), 1_069_014_363_689);
    }

    #[test]
    fn shuffle_matches_cpython() {
        assert_eq!(precommit_shuffled_indices(0).len(), 0);
        assert_eq!(precommit_shuffled_indices(1), vec![0]);
        assert_eq!(precommit_shuffled_indices(2), vec![0, 1]);
        assert_eq!(precommit_shuffled_indices(5), vec![1, 0, 2, 3, 4]);
        assert_eq!(
            precommit_shuffled_indices(10),
            vec![4, 0, 5, 1, 8, 6, 2, 3, 7, 9]
        );
        assert_eq!(
            precommit_shuffled_indices(17),
            vec![1, 9, 4, 12, 11, 0, 13, 16, 2, 8, 3, 7, 10, 14, 5, 6, 15]
        );
        assert_eq!(
            precommit_shuffled_indices(64),
            vec![
                19, 52, 28, 36, 29, 9, 51, 46, 24, 42, 14, 10, 58, 38, 30, 18, 11, 12, 33, 7, 47,
                62, 35, 37, 44, 6, 1, 0, 17, 32, 53, 3, 54, 2, 22, 55, 56, 43, 60, 50, 23, 4, 59,
                25, 16, 48, 63, 27, 49, 5, 45, 26, 40, 8, 34, 15, 31, 57, 41, 21, 20, 13, 39, 61
            ]
        );
        let v = precommit_shuffled_indices(1000);
        assert_eq!(v[..10], [757, 225, 304, 112, 445, 132, 446, 113, 928, 284]);
        assert_eq!(v[990..], [240, 496, 920, 670, 338, 320, 210, 625, 491, 589]);
        let weighted: u64 = v
            .iter()
            .enumerate()
            .map(|(i, &x)| u64::try_from(i).unwrap() * u64::from(x))
            .sum();
        assert_eq!(weighted, 249_781_307);
    }
}
