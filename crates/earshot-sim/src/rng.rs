/// A small, fast, seedable generator (xorshift64*), so traces are
/// reproducible in tests and benchmarks.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Any seed works, zero included.
    pub fn new(seed: u64) -> Self {
        // SplitMix64 spreads close seeds apart and avoids the all-zero state.
        let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Rng((z ^ (z >> 31)) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `0..1`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_runs_the_same_way_whatever_it_is() {
        for seed in [0, 1, 2, u64::MAX, 0x9e37_79b9_7f4a_7c15] {
            let (mut a, mut b) = (Rng::new(seed), Rng::new(seed));
            for _ in 0..100 {
                assert_eq!(a.next_u64(), b.next_u64());
                assert_eq!(a.next_f64(), b.next_f64());
            }
        }
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn fractions_fill_the_unit_interval_evenly() {
        let mut rng = Rng::new(7);
        let mut sum = 0.0f64;
        for _ in 0..100_000 {
            let x = rng.next_f64();
            assert!((0.0..1.0).contains(&x));
            sum += x;
        }
        assert!((sum / 100_000.0 - 0.5).abs() < 0.01);
    }
}
