//! Deterministic random numbers for the randomizer.
//!
//! The generator is xoshiro256** (Blackman and Vigna, public domain) with its
//! state filled from SplitMix64. Both are tiny, have published reference
//! outputs (checked in the tests below) and behave identically on every
//! platform, including wasm32, so a seed produces the same ROM everywhere.
//!
//! Rules for users of this module:
//!
//! * Draw only through [`Rng`]. Never use `HashMap` iteration order, the
//!   system clock or thread-local randomness for anything that reaches the
//!   output.
//! * Bounded draws ([`Rng::below`], [`Rng::range`]) are unbiased (Lemire's
//!   multiply-shift with rejection), so changing a bound never needs a
//!   modulo fix-up.
//! * Each pipeline module gets its own stream ([`Rng::derive`]), so a change
//!   in how one module draws does not reshuffle every module after it.

/// SplitMix64 step: advances `state` and returns the next output.
#[must_use]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 64-bit FNV-1a over `bytes`, used to turn labels into stream ids.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// xoshiro256** generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seed from one 64-bit value through SplitMix64 (the reference way to
    /// fill xoshiro state). An all-zero state cannot come out of SplitMix64
    /// for four consecutive outputs, but it is guarded anyway.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let mut s = [0u64; 4];
        for v in &mut s {
            *v = splitmix64(&mut sm);
        }
        if s == [0; 4] {
            s[0] = 1;
        }
        Rng { s }
    }

    /// Build directly from a raw state (tests and reference vectors).
    #[must_use]
    pub fn from_state(s: [u64; 4]) -> Self {
        Rng { s }
    }

    /// Current raw state.
    #[must_use]
    pub fn state(&self) -> [u64; 4] {
        self.s
    }

    /// An independent stream for `label`, derived from this generator's
    /// current state without advancing it.
    #[must_use]
    pub fn derive(&self, label: &str) -> Rng {
        let mut seed = fnv1a64(label.as_bytes());
        for w in self.s {
            seed = seed.rotate_left(17) ^ w;
            let mut t = seed;
            seed = splitmix64(&mut t);
        }
        Rng::new(seed)
    }

    /// Next raw 64-bit output.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Next 32-bit output (the high half, the better-mixed bits).
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform integer in `0..n` without modulo bias. `n == 0` returns 0
    /// without drawing.
    pub fn below(&mut self, n: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        // Lemire: multiply into 128 bits, reject the few low products that
        // would make some results more likely than others.
        let threshold = n.wrapping_neg() % n;
        loop {
            let m = u128::from(self.next_u64()) * u128::from(n);
            if (m as u64) >= threshold {
                return (m >> 64) as u64;
            }
        }
    }

    /// Uniform `usize` in `0..n` (`n == 0` returns 0).
    pub fn index(&mut self, n: usize) -> usize {
        self.below(n as u64) as usize
    }

    /// Uniform integer in the inclusive range `lo..=hi`. Swapped bounds are
    /// put in order.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        let span = (hi as i128 - lo as i128 + 1) as u128;
        if span > u128::from(u64::MAX) {
            return self.next_u64() as i64;
        }
        lo + self.below(span as u64) as i64
    }

    /// Uniform `u8` in `lo..=hi`.
    pub fn range_u8(&mut self, lo: u8, hi: u8) -> u8 {
        self.range(i64::from(lo), i64::from(hi)) as u8
    }

    /// Uniform float in `[0, 1)` with 53 random bits.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// `true` with probability `num / den` (exact integer test, no floats).
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        if den == 0 {
            return false;
        }
        self.below(den) < num
    }

    /// Fair coin.
    pub fn coin(&mut self) -> bool {
        self.next_u64() >> 63 == 1
    }

    /// Fisher-Yates shuffle in place (forward form: position `i` takes a
    /// uniform pick from `i..len`).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        let n = items.len();
        for i in 0..n.saturating_sub(1) {
            let j = i + self.index(n - i);
            items.swap(i, j);
        }
    }

    /// Uniform element, or `None` (without drawing) when `items` is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[self.index(items.len())])
        }
    }

    /// Index chosen with probability proportional to `weights[i]`. Zero
    /// weights are never chosen; `None` (without drawing) when every weight
    /// is zero or the slice is empty.
    pub fn weighted_index(&mut self, weights: &[u32]) -> Option<usize> {
        let total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
        if total == 0 {
            return None;
        }
        let mut roll = self.below(total);
        for (i, &w) in weights.iter().enumerate() {
            let w = u64::from(w);
            if roll < w {
                return Some(i);
            }
            roll -= w;
        }
        None
    }

    /// Element of `items` picked by weight (see [`Rng::weighted_index`]).
    pub fn weighted_pick<'a, T>(&mut self, items: &'a [(T, u32)]) -> Option<&'a T> {
        let weights: Vec<u32> = items.iter().map(|(_, w)| *w).collect();
        self.weighted_index(&weights).map(|i| &items[i].0)
    }

    /// `k` distinct indices from `0..n` in random order (`k` is clamped to `n`).
    pub fn sample_indices(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut all: Vec<usize> = (0..n).collect();
        let k = k.min(n);
        for i in 0..k {
            let j = i + self.index(n - i);
            all.swap(i, j);
        }
        all.truncate(k);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_outputs() {
        // Published SplitMix64 outputs for seed 0.
        let mut s = 0u64;
        assert_eq!(splitmix64(&mut s), 0xE220_A839_7B1D_CDAF);
        assert_eq!(splitmix64(&mut s), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(splitmix64(&mut s), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn xoshiro256starstar_reference_outputs() {
        // Reference sequence for state [1, 2, 3, 4] (same as the vectors used
        // by other xoshiro256** implementations).
        let mut r = Rng::from_state([1, 2, 3, 4]);
        let want: [u64; 6] = [
            11520,
            0,
            1_509_978_240,
            1_215_971_899_390_074_240,
            1_216_172_134_540_287_360,
            607_988_272_756_665_600,
        ];
        for w in want {
            assert_eq!(r.next_u64(), w);
        }
    }

    #[test]
    fn same_seed_same_stream_and_derive_is_pure() {
        let a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut a2 = a.clone();
        for _ in 0..100 {
            assert_eq!(a2.next_u64(), b.next_u64());
        }
        let d1 = a.derive("palaces");
        let d2 = a.derive("palaces");
        assert_eq!(d1, d2);
        assert_ne!(a.derive("palaces"), a.derive("items"));
        assert_eq!(a, Rng::new(42), "derive does not advance the parent");
    }

    #[test]
    fn below_stays_in_bounds_and_hits_every_value() {
        let mut r = Rng::new(7);
        for n in [1u64, 2, 3, 5, 7, 10, 255, 1000] {
            let mut seen = vec![false; n as usize];
            for _ in 0..(n * 50).max(100) {
                let v = r.below(n);
                assert!(v < n);
                seen[v as usize] = true;
            }
            assert!(seen.iter().all(|&s| s), "n={n} missed a value");
        }
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn below_is_roughly_uniform() {
        let mut r = Rng::new(99);
        let mut counts = [0u32; 3];
        for _ in 0..30_000 {
            counts[r.below(3) as usize] += 1;
        }
        for c in counts {
            assert!((9_000..11_000).contains(&c), "{counts:?}");
        }
    }

    #[test]
    fn range_is_inclusive_and_orders_bounds() {
        let mut r = Rng::new(1);
        let mut lo_seen = false;
        let mut hi_seen = false;
        for _ in 0..1000 {
            let v = r.range(5, 7);
            assert!((5..=7).contains(&v));
            lo_seen |= v == 5;
            hi_seen |= v == 7;
            let w = r.range(-3, -5);
            assert!((-5..=-3).contains(&w));
        }
        assert!(lo_seen && hi_seen);
        assert_eq!(r.range(4, 4), 4);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = Rng::new(3);
        let mut v: Vec<u32> = (0..50).collect();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(v, sorted, "50 elements essentially never stay in order");
    }

    #[test]
    fn weighted_pick_respects_zero_and_proportions() {
        let mut r = Rng::new(11);
        assert_eq!(r.weighted_index(&[]), None);
        assert_eq!(r.weighted_index(&[0, 0]), None);
        let mut counts = [0u32; 3];
        for _ in 0..40_000 {
            counts[r.weighted_index(&[1, 0, 3]).unwrap()] += 1;
        }
        assert_eq!(counts[1], 0);
        assert!((9_000..11_000).contains(&counts[0]), "{counts:?}");
        assert!((29_000..31_000).contains(&counts[2]), "{counts:?}");
        let items = [("a", 0u32), ("b", 5)];
        assert_eq!(r.weighted_pick(&items), Some(&"b"));
    }

    #[test]
    fn sample_indices_are_distinct() {
        let mut r = Rng::new(5);
        let s = r.sample_indices(10, 4);
        assert_eq!(s.len(), 4);
        let mut d = s.clone();
        d.sort_unstable();
        d.dedup();
        assert_eq!(d.len(), 4);
        assert_eq!(r.sample_indices(3, 10).len(), 3);
    }

    #[test]
    fn f64_and_chance_bounds() {
        let mut r = Rng::new(8);
        for _ in 0..1000 {
            let f = r.next_f64();
            assert!((0.0..1.0).contains(&f));
        }
        assert!(!r.chance(0, 4));
        assert!(r.chance(4, 4));
        assert!(!r.chance(1, 0));
    }
}
