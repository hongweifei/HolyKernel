//! 文件系统后端能力。

/// 后端能力位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps(u32);

impl Caps {
    /// 支持集合与名字关系。
    pub const COLLECTIONS: Self = Self(1 << 0);
    /// 支持不可变修订链。
    pub const REVISIONS: Self = Self(1 << 1);
    /// 支持快照。
    pub const SNAPSHOTS: Self = Self(1 << 2);
    /// 支持标签。
    pub const TAGS: Self = Self(1 << 3);
    /// 支持按偏移写入。
    pub const RANGE_WRITE: Self = Self(1 << 4);
    /// 外来文件系统；修订为合成值。
    pub const FOREIGN: Self = Self(1 << 5);
    /// 修订没有历史，只有 head。
    pub const WEAK_REVISIONS: Self = Self(1 << 6);

    /// 原生 Holy 能力。
    pub const NATIVE: Self = Self(
        Self::COLLECTIONS.0
            | Self::REVISIONS.0
            | Self::SNAPSHOTS.0
            | Self::TAGS.0
            | Self::RANGE_WRITE.0,
    );

    /// 外来路径文件系统的最低能力。
    pub const PATH: Self =
        Self(Self::COLLECTIONS.0 | Self::FOREIGN.0 | Self::WEAK_REVISIONS.0 | Self::RANGE_WRITE.0);

    /// 空能力。
    pub const fn empty() -> Self {
        Self(0)
    }

    /// 构造能力集合。
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// 返回位掩码。
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// 判断是否包含全部能力。
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    /// 判断任意一项能力是否存在。
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl core::ops::BitOr for Caps {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
