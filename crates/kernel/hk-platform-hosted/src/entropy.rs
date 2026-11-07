//! 确定性熵源。

use std::sync::{Mutex, MutexGuard};

use hk_hal::Entropy;

/// 由种子驱动的 SplitMix64 熵源。
///
/// 相同种子产生相同字节序列，支撑确定性回放。
#[derive(Debug)]
pub struct HostedEntropy {
    state: Mutex<u64>,
}

impl HostedEntropy {
    /// 以种子构造熵源。
    pub fn new(seed: u64) -> Self {
        Self {
            state: Mutex::new(seed),
        }
    }

    fn lock(&self) -> MutexGuard<'_, u64> {
        self.state.lock().expect("熵源锁中毒")
    }

    fn next_u64(&self) -> u64 {
        let mut state = self.lock();
        let mixed = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        *state = mixed;
        let mut value = mixed;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}

impl Entropy for HostedEntropy {
    fn fill(&self, buffer: &mut [u8]) {
        for chunk in buffer.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_repeats() {
        let first = HostedEntropy::new(7);
        let second = HostedEntropy::new(7);
        let mut left = [0_u8; 32];
        let mut right = [0_u8; 32];

        first.fill(&mut left);
        second.fill(&mut right);

        assert_eq!(left, right);
    }

    #[test]
    fn different_seeds_differ() {
        let first = HostedEntropy::new(7);
        let second = HostedEntropy::new(8);
        let mut left = [0_u8; 32];
        let mut right = [0_u8; 32];

        first.fill(&mut left);
        second.fill(&mut right);

        assert_ne!(left, right);
    }
}
