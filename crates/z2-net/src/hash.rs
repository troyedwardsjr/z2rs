//! FNV-1a 64 state hashing for desync detection.

/// FNV-1a 64 offset basis.
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 prime.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a 64 hasher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv64 {
    /// Fresh hasher at the offset basis.
    pub const fn new() -> Self {
        Self(FNV_OFFSET)
    }

    /// Mix bytes in.
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// Mix a little-endian u32 in.
    pub fn write_u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }

    /// Current hash value.
    pub const fn finish(&self) -> u64 {
        self.0
    }
}

/// FNV-1a 64 over the concatenation of `parts`, e.g. `hash_state(&[ram, wram, oam])`.
pub fn hash_state(parts: &[&[u8]]) -> u64 {
    let mut h = Fnv64::new();
    for p in parts {
        h.write(p);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(hash_state(&[]), FNV_OFFSET);
        // FNV-1a 64 of "a" is 0xaf63dc4c8601ec8c.
        assert_eq!(hash_state(&[b"a"]), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash_state(&[b"ab", b"c"]), hash_state(&[b"abc"]));
    }
}
