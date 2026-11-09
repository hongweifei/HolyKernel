//! IOMMU 域映射。

use core::fmt;

use bitflags::bitflags;

use crate::addr::{PAGE_SIZE, PhysRange};

/// IOMMU 域标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IommuDomain(u32);

impl IommuDomain {
    /// 构造域标识。
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// 返回域标识。
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// 设备可见的 I/O 虚拟地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Iova(u64);

impl Iova {
    /// 构造 IOVA。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回 IOVA 数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

bitflags! {
    /// IOMMU 映射权限。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct IommuFlags: u8 {
        /// 设备可读。
        const READ = 1 << 0;
        /// 设备可写。
        const WRITE = 1 << 1;
    }
}

/// IOMMU 操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IommuError {
    /// 平台没有 IOMMU。
    Unsupported,
    /// 地址或长度未按页对齐。
    Unaligned,
    /// IOVA 已有映射。
    AlreadyMapped,
    /// IOVA 没有映射。
    NotMapped,
    /// 地址空间不足。
    OutOfMemory,
}

impl fmt::Display for IommuError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("平台不支持 IOMMU"),
            Self::Unaligned => formatter.write_str("IOMMU 地址未按页对齐"),
            Self::AlreadyMapped => formatter.write_str("IOVA 已有映射"),
            Self::NotMapped => formatter.write_str("IOVA 没有映射"),
            Self::OutOfMemory => formatter.write_str("IOMMU 地址空间不足"),
        }
    }
}

impl core::error::Error for IommuError {}

/// IOMMU 域映射接口。
pub trait Iommu: Send + Sync {
    /// 把物理区间映射到设备 IOVA。
    fn map(
        &self,
        domain: IommuDomain,
        iova: Iova,
        phys: PhysRange,
        flags: IommuFlags,
    ) -> Result<(), IommuError>;

    /// 解除 IOVA 映射。
    fn unmap(&self, domain: IommuDomain, iova: Iova, len: usize) -> Result<(), IommuError>;

    /// 刷新该域的 IOTLB。
    fn flush(&self, domain: IommuDomain) -> Result<(), IommuError>;

    /// 查询 IOVA 对应的物理区间与权限；软件/真实后端都可实现。
    fn translate(&self, _domain: IommuDomain, _iova: Iova) -> Option<(PhysRange, IommuFlags)> {
        None
    }

    /// 该后端是否真的会把设备访问的 IOVA 翻译为物理地址。
    ///
    /// 软件 IOMMU 只维护映射账本，设备仍使用物理地址，因此返回 `false`；
    /// VT-d 等硬件后端返回 `true` 后，运行时才会把窗口内的 IOVA 交给驱动。
    fn translates_device_addresses(&self) -> bool {
        false
    }
}

/// 没有 IOMMU 的平台实现。
pub struct NoIommu;

impl Iommu for NoIommu {
    fn map(
        &self,
        _domain: IommuDomain,
        _iova: Iova,
        _phys: PhysRange,
        _flags: IommuFlags,
    ) -> Result<(), IommuError> {
        Err(IommuError::Unsupported)
    }

    fn unmap(&self, _domain: IommuDomain, _iova: Iova, _len: usize) -> Result<(), IommuError> {
        Err(IommuError::Unsupported)
    }

    fn flush(&self, _domain: IommuDomain) -> Result<(), IommuError> {
        Err(IommuError::Unsupported)
    }
}

/// 无硬件 IOMMU 时的 identity 映射后端。
pub struct IdentityIommu;

impl Iommu for IdentityIommu {
    fn map(
        &self,
        _domain: IommuDomain,
        iova: Iova,
        phys: PhysRange,
        _flags: IommuFlags,
    ) -> Result<(), IommuError> {
        if !iova.get().is_multiple_of(PAGE_SIZE as u64)
            || !phys.start().get().is_multiple_of(PAGE_SIZE as u64)
            || !phys.len().is_multiple_of(PAGE_SIZE as u64)
        {
            return Err(IommuError::Unaligned);
        }
        if iova.get() != phys.start().get() {
            return Err(IommuError::Unsupported);
        }
        Ok(())
    }

    fn unmap(&self, _domain: IommuDomain, iova: Iova, len: usize) -> Result<(), IommuError> {
        if !iova.get().is_multiple_of(PAGE_SIZE as u64) || len == 0 {
            return Err(IommuError::Unaligned);
        }
        Ok(())
    }

    fn flush(&self, _domain: IommuDomain) -> Result<(), IommuError> {
        Ok(())
    }
}
