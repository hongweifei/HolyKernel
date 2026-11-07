//! CRC32C（Castagnoli）校验。

/// 计算 CRC32C。
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn matches_known_vector() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }
}
