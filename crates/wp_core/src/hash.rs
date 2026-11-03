//! FNV-1a 64-bit: a tiny hash whose output never changes between Rust
//! versions or platforms, unlike `std::hash::DefaultHasher`.

#[derive(Clone, Debug)]
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }
    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Hash bytes and format as 16 hex digits.
pub fn hex_hash(bytes: &[u8]) -> String {
    let mut h = StableHasher::new();
    h.write(bytes);
    format!("{:016x}", h.finish())
}
