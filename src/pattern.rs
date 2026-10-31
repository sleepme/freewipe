//! A single overwrite pass and the buffer-fill primitive.
//!
//! Platform-agnostic. The I/O layer repeatedly fills a reusable buffer with
//! [`fill`] and writes it to the target; nothing here touches the filesystem.

use rand::RngCore;

/// One overwrite pass over the full length of the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Fill with a single repeated byte (e.g. `0x00`, `0xFF`, `0xAA`).
    Fixed(u8),
    /// Fill with cryptographically strong random data.
    Random,
    /// Fill by repeating a fixed multi-byte pattern, aligned to the start of
    /// the stream (Gutmann's encoded patterns, custom methods).
    Pattern(std::borrow::Cow<'static, [u8]>),
}

impl Pass {
    /// Short human-readable label for progress output and logs.
    pub fn label(&self) -> String {
        match self {
            Pass::Fixed(b) => format!("0x{b:02X}"),
            Pass::Random => "random".to_string(),
            Pass::Pattern(p) => {
                let hex: Vec<String> = p.iter().map(|b| format!("{b:02X}")).collect();
                format!("pattern[{}]", hex.join(" "))
            }
        }
    }

    /// Whether this pass writes attacker-unpredictable data. Verification of a
    /// random pass requires reading back and comparing against what was
    /// actually written, not against a known constant.
    pub fn is_random(&self) -> bool {
        matches!(self, Pass::Random)
    }
}

/// Fill `buf` entirely according to `pass`.
///
/// `offset` is the absolute byte position of `buf[0]` within the target
/// stream, so that a [`Pass::Pattern`] stays phase-aligned across successive
/// buffers (important for the multi-byte Gutmann patterns).
pub fn fill(buf: &mut [u8], pass: &Pass, offset: u64, rng: &mut dyn RngCore) {
    match pass {
        Pass::Fixed(b) => buf.fill(*b),
        Pass::Random => rng.fill_bytes(buf),
        Pass::Pattern(p) => {
            let p: &[u8] = p;
            if p.is_empty() {
                buf.fill(0);
                return;
            }
            let plen = p.len() as u64;
            for (i, byte) in buf.iter_mut().enumerate() {
                let pos = (offset.wrapping_add(i as u64)) % plen;
                *byte = p[pos as usize];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn fixed_fills_constant() {
        let mut buf = [1u8; 64];
        let mut rng = ChaCha20Rng::seed_from_u64(0);
        fill(&mut buf, &Pass::Fixed(0xAB), 0, &mut rng);
        assert!(buf.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn pattern_is_phase_aligned_across_buffers() {
        // 3-byte pattern split across two buffers must stay continuous.
        static P: &[u8] = &[0x92, 0x49, 0x24];
        let mut rng = ChaCha20Rng::seed_from_u64(0);

        let mut whole = [0u8; 9];
        fill(&mut whole, &Pass::Pattern(std::borrow::Cow::Borrowed(P)), 0, &mut rng);

        let mut a = [0u8; 4];
        let mut b = [0u8; 5];
        fill(&mut a, &Pass::Pattern(std::borrow::Cow::Borrowed(P)), 0, &mut rng);
        fill(&mut b, &Pass::Pattern(std::borrow::Cow::Borrowed(P)), 4, &mut rng);

        let mut joined = Vec::new();
        joined.extend_from_slice(&a);
        joined.extend_from_slice(&b);
        assert_eq!(whole.as_slice(), joined.as_slice());
        assert_eq!(&whole[0..3], P);
        assert_eq!(&whole[3..6], P);
    }

    #[test]
    fn random_is_not_constant() {
        let mut buf = [0u8; 4096];
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        fill(&mut buf, &Pass::Random, 0, &mut rng);
        // Astronomically unlikely to be all-equal if truly filled.
        assert!(buf.windows(2).any(|w| w[0] != w[1]));
    }
}
