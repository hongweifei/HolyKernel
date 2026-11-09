//! CPU 亲和性掩码。

use crate::CadenceError;

/// 掩码可表示的最大 CPU 数。
pub const MAX_CPUS: u32 = 64;

/// CPU 亲和性掩码。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CpuMask(u64);

impl CpuMask {
    /// 全部 CPU。
    pub const ALL: Self = Self(u64::MAX);

    /// 空掩码。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 全部 CPU。
    pub const fn all() -> Self {
        Self::ALL
    }

    /// 以位图构造。
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// 只包含一个 CPU。
    pub const fn single(cpu: u32) -> Option<Self> {
        if cpu >= MAX_CPUS {
            return None;
        }
        Some(Self(1_u64 << cpu))
    }

    /// 返回位图。
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// 判断是否为空。
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// 判断是否包含指定 CPU。
    pub const fn contains(self, cpu: u32) -> bool {
        cpu < MAX_CPUS && self.0 & (1_u64 << cpu) != 0
    }

    /// 返回 CPU 数量。
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// 返回交集。
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// 把掩码收窄为 `parent` 的子集。
    pub const fn restrict(self, parent: Self) -> Result<Self, CadenceError> {
        if self.0 & !parent.0 != 0 {
            return Err(CadenceError::AffinityNotSubset);
        }
        if self.0 == 0 {
            return Err(CadenceError::ZeroCpuMask);
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_and_queries_masks() {
        let two = CpuMask::single(2).unwrap();
        assert!(two.contains(2));
        assert!(!two.contains(3));
        assert_eq!(two.count(), 1);
        assert_eq!(CpuMask::single(64), None);

        let combined = CpuMask::from_bits(two.bits() | CpuMask::single(5).unwrap().bits());
        assert_eq!(combined.count(), 2);
        assert_eq!(combined.intersect(two), two);
    }

    #[test]
    fn restricts_to_parent_only() {
        let parent = CpuMask::from_bits(0b0110);
        let child = CpuMask::from_bits(0b0010);
        assert_eq!(child.restrict(parent), Ok(child));
        assert_eq!(
            CpuMask::from_bits(0b0001).restrict(parent).err(),
            Some(CadenceError::AffinityNotSubset)
        );
        assert_eq!(
            CpuMask::empty().restrict(parent).err(),
            Some(CadenceError::ZeroCpuMask)
        );
    }
}
