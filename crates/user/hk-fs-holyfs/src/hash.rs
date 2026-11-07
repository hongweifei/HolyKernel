//! 内容哈希：用于修订完整性与去重。

/// FNV-1a 64 位初始值。
const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 位质数。
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// 增量内容哈希；按块喂入时不额外分配整段内容。
#[derive(Clone, Copy)]
pub(crate) struct ContentHasher {
    state: u64,
}

impl ContentHasher {
    pub(crate) const fn new() -> Self {
        Self {
            state: OFFSET_BASIS,
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        for byte in data {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(PRIME);
        }
    }

    pub(crate) const fn finish(self) -> u64 {
        self.state
    }
}

/// 计算内容哈希；FNV-1a 64 位，稳定且无需额外表。
pub(crate) fn content_hash(data: &[u8]) -> u64 {
    let mut hasher = ContentHasher::new();
    hasher.update(data);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::{ContentHasher, content_hash};

    #[test]
    fn hashes_are_stable() {
        assert_eq!(content_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(content_hash(b"holy"), content_hash(b"holy"));
        assert_ne!(content_hash(b"holy"), content_hash(b"hole"));
    }

    #[test]
    fn incremental_matches_single_pass() {
        let data = [0x5A_u8; 4096];
        let mut hasher = ContentHasher::new();
        hasher.update(&data[..1000]);
        hasher.update(&data[1000..]);
        assert_eq!(hasher.finish(), content_hash(&data));
    }
}
