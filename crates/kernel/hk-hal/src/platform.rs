//! 平台抽象。

use core::fmt;

use crate::arch::{Arch, CpuId};
use crate::boot::{BootInfo, BootInfoError};
use crate::cache::CacheOps;
use crate::console::Console;
use crate::dma::DmaOps;
use crate::entropy::Entropy;
use crate::iommu::Iommu;
use crate::irq::InterruptController;
use crate::mmu::Mmu;
use crate::physical::PhysicalAccess;
use crate::time::Timer;
use hk_device::PciConfig;

/// 逻辑 CPU 的固件拓扑。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTopology {
    /// ACPI 处理器 UID。
    pub acpi_id: u32,
    /// PPTT 物理封装编号；未知时为 0。
    pub package_id: u32,
    /// SRAT 邻近域；未知时为 0。
    pub proximity: u32,
}

/// 平台初始化错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformError {
    /// 启动信息校验失败。
    BootInfo(BootInfoError),
    /// CPU 标识超出平台核数。
    CpuOutOfRange {
        /// 请求的 CPU 标识。
        cpu: CpuId,
    },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BootInfo(error) => write!(formatter, "启动信息校验失败：{error}"),
            Self::CpuOutOfRange { cpu } => write!(formatter, "CPU 标识超出范围：{}", cpu.get()),
        }
    }
}

impl core::error::Error for PlatformError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::BootInfo(error) => Some(error),
            Self::CpuOutOfRange { .. } => None,
        }
    }
}

/// 平台。
///
/// 每个目标只有一个平台实现；内核、Root Server 与用户态服务器通过该 trait
/// 访问 CPU、MMU、中断、定时器、控制台、DMA 与熵源。
pub trait Platform: Send + Sync {
    /// 返回平台名称。
    fn name(&self) -> &'static str;

    /// 返回平台架构。
    fn arch(&self) -> Arch;

    /// 返回可用 CPU 数量。
    fn cpu_count(&self) -> usize;

    /// 完成平台初始化，校验启动信息。
    fn init(&self, boot: &BootInfo) -> Result<(), PlatformError>;

    /// 初始化指定 CPU 的本地状态。
    fn init_cpu(&self, cpu: CpuId) -> Result<(), PlatformError>;

    /// 关闭系统，不返回。
    fn shutdown(&self) -> !;

    /// 重启系统，不返回。
    fn reboot(&self) -> !;

    /// 返回内存管理单元。
    fn mmu(&self) -> &dyn Mmu;

    /// 返回中断控制器。
    fn irq(&self) -> &dyn InterruptController;

    /// 返回定时器。
    fn timer(&self) -> &dyn Timer;

    /// 返回控制台。
    fn console(&self) -> &dyn Console;

    /// 返回 PCI 配置空间访问；平台没有 PCI 时返回 `None`。
    fn pci(&self) -> Option<&dyn PciConfig>;

    /// 返回 DMA 通道。
    fn dma(&self) -> &dyn DmaOps;

    /// 返回 direct map 物理内存访问通道。
    fn physical(&self) -> &dyn PhysicalAccess;

    /// 返回熵源。
    fn entropy(&self) -> &dyn Entropy;

    /// 返回 IOMMU；平台没有时返回 `None`。
    fn iommu(&self) -> Option<&dyn Iommu> {
        None
    }

    /// 返回缓存维护接口；平台没有时返回 `None`。
    fn cache(&self) -> Option<&dyn CacheOps> {
        None
    }

    /// 返回当前 CPU 温度（摄氏度）；平台不支持时返回 `None`。
    fn temperature_celsius(&self) -> Option<u8> {
        None
    }

    /// 返回逻辑 CPU 的 ACPI/SRAT/PPTT 拓扑；平台不支持时返回 `None`。
    fn cpu_topology(&self, _cpu: CpuId) -> Option<CpuTopology> {
        None
    }
}
