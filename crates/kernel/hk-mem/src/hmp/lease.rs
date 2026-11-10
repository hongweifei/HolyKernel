//! FrameLease：类型化能力引用的外部物理区间。

use hk_hal::{PhysAddr, PhysRange};

/// 一段由设备或外部来源导入的物理区间；不归还 Capacity。
pub struct FrameLease {
    phys: PhysAddr,
    len: usize,
}

impl FrameLease {
    /// 构造导入的物理区间租约；释放时只丢弃描述，不归还物理页。
    pub const fn import(phys: PhysAddr, len: usize) -> Self {
        Self { phys, len }
    }

    /// 返回起始物理地址。
    pub const fn phys(&self) -> PhysAddr {
        self.phys
    }

    /// 返回字节长度。
    pub const fn len(&self) -> usize {
        self.len
    }

    /// 判断长度是否为零。
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回物理区间。
    pub const fn range(&self) -> PhysRange {
        PhysRange::new(self.phys, self.len as u64)
    }
}
