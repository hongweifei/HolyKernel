//! 内容哈希：用于修订完整性与去重。

/// 计算内容哈希；FNV-1a 64 位，稳定且无需额外表。
pub(crate) fn content_hash(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::content_hash;

    #[test]
    fn hashes_are_stable() {
        assert_eq!(content_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(content_hash(b"holy"), content_hash(b"holy"));
        assert_ne!(content_hash(b"holy"), content_hash(b"hole"));
    }
}
