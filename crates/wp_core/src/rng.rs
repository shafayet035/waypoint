//! A seeded RNG with identical output on every platform (SplitMix64 seeding a
//! xoshiro256** generator). Use one per subsystem, seeded from the scenario seed.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimRng {
    s: [u64; 4],
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl SimRng {
    pub fn new(seed: u64) -> Self {
        let mut st = seed;
        Self {
            s: [
                splitmix64(&mut st),
                splitmix64(&mut st),
                splitmix64(&mut st),
                splitmix64(&mut st),
            ],
        }
    }

    /// Derive an independent stream for a named subsystem.
    pub fn fork(seed: u64, subsystem: &str) -> Self {
        let mut h = crate::hash::StableHasher::new();
        h.write_u64(seed);
        h.write(subsystem.as_bytes());
        Self::new(h.finish())
    }

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

    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in [lo, hi).
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = SimRng::new(7);
        let mut b = SimRng::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert_ne!(SimRng::new(7).next_u64(), SimRng::new(8).next_u64());
    }

    #[test]
    fn floats_in_range() {
        let mut r = SimRng::new(1);
        for _ in 0..1000 {
            let x = r.next_f32();
            assert!((0.0..1.0).contains(&x));
        }
    }
}
