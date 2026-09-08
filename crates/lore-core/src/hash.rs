//! Canonical hashing algorithms and utilities for content fingerprints and deterministic identifiers.
//!
//! Lore uses 64-bit FNV-1a hex digests (`016x`, 16 hex characters) for fast inline
//! source-artifact fingerprinting (`full_hash` and `prefix_hash`), deterministic
//! entity IDs (`det_id`), secret fingerprints, and job deduplication. Cryptographic
//! hashing (BLAKE3) is used for blob addresses (see `DATA_MODEL.md` §3 and §6).

/// 64-bit FNV-1a offset basis.
pub const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// 64-bit FNV-1a prime.
pub const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental 64-bit FNV-1a hasher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fnv1a64 {
    state: u64,
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv1a64 {
    /// Create a new hasher initialized with the standard FNV-1a 64-bit offset basis.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: FNV1A64_OFFSET,
        }
    }

    /// Create a new hasher initialized with a custom seed xor'd with the offset basis.
    #[must_use]
    pub const fn with_seed(seed: u64) -> Self {
        Self {
            state: FNV1A64_OFFSET ^ seed,
        }
    }

    /// Feed a byte slice into the hasher.
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(FNV1A64_PRIME);
        }
    }

    /// Feed a single byte into the hasher.
    pub fn update_byte(&mut self, byte: u8) {
        self.state ^= u64::from(byte);
        self.state = self.state.wrapping_mul(FNV1A64_PRIME);
    }

    /// Return the computed 64-bit hash.
    #[must_use]
    pub const fn finish(self) -> u64 {
        self.state
    }

    /// Return the computed hash as a 16-character lowercase hexadecimal string (`016x`).
    #[must_use]
    pub fn finish_hex(self) -> String {
        format!("{:016x}", self.state)
    }
}

/// Compute the 64-bit FNV-1a hash of a byte slice.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a64::new();
    hasher.update(bytes);
    hasher.finish()
}

/// Compute the 64-bit FNV-1a hex digest (`016x`) of a byte slice.
#[must_use]
pub fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hasher = Fnv1a64::new();
    hasher.update(bytes);
    hasher.finish_hex()
}

/// Deterministic opaque identifier derived from a prefix and stable natural-key parts,
/// separated by unit separator (`0x1f`).
#[must_use]
pub fn det_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Fnv1a64::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update_byte(0x1f);
        }
        hasher.update(part.as_bytes());
    }
    format!("{prefix}_{}", hasher.finish_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slice_matches_fnv_offset() {
        assert_eq!(fnv1a64(b""), FNV1A64_OFFSET);
        assert_eq!(fnv1a64_hex(b""), format!("{FNV1A64_OFFSET:016x}"));
    }

    #[test]
    fn known_vector_test() {
        let digest = fnv1a64_hex(b"hello world");
        assert_eq!(digest.len(), 16);
        assert_eq!(fnv1a64(b"hello world"), 0x779a_65e7_023c_d2e7);
        assert_eq!(digest, "779a65e7023cd2e7");
    }

    #[test]
    fn det_id_combines_parts_with_separator() {
        let id1 = det_id("job", &["a", "b"]);
        let id2 = det_id("job", &["a", "b"]);
        let id3 = det_id("job", &["ab"]);
        assert_eq!(id1, id2);
        assert_ne!(id1, id3, "unit separator must prevent boundary ambiguity");
        assert!(id1.starts_with("job_"));
        assert_eq!(id1.len(), "job_".len() + 16);
    }

    #[test]
    fn with_seed_differs_from_default() {
        let h1 = Fnv1a64::new();
        let h2 = Fnv1a64::with_seed(42);
        assert_ne!(h1.finish(), h2.finish());
    }
}
