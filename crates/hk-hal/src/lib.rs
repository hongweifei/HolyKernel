//! HolyKernel 硬件抽象层。
//!
//! 内核只依赖本 crate 的 trait 访问平台；架构差异位于 `hk-platform-*` 实现。
//! 地址、架构标识与启动信息由 [`hk_base`] 定义并在此重导出。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod cache;
pub mod console;
pub mod device;
pub mod dma;
pub mod entropy;
pub mod execution;
pub mod io;
pub mod iommu;
pub mod irq;
pub mod mmu;
pub mod pci;
pub mod physical;
pub mod platform;
pub mod power;
pub mod time;
pub mod user;
pub mod user_memory;

/// 支持的 CPU 数量上限。
pub const MAX_CPUS: usize = 16;

pub use cache::{CacheOps, NoCacheOps};
pub use console::Console;
pub use device::{DEVICE_GRANT_BYTES, DeviceGrant, IoRange, MmioRange};
pub use dma::{DmaBuffer, DmaError, DmaOps};
pub use entropy::Entropy;
pub use execution::{SyscallFrame, UserContext};
pub use hk_base::{
    Arch, BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootInfoError, BootModule, CpuId,
    DirectMapInfo, FramebufferInfo, MemoryRegion, PAGE_SHIFT, PAGE_SIZE, PhysAddr, PhysRange,
    PixelFormat, RegionKind, VirtAddr,
};
pub use hk_base::{addr, arch, boot};
pub use io::Io;
pub use iommu::{IdentityIommu, Iommu, IommuDomain, IommuError, IommuFlags, Iova, NoIommu};
pub use irq::{InterruptController, Ipi, Irq, IrqBitmap, IrqError, NotificationId};
pub use mmu::{MapFlags, Mmu, MmuError, VSpaceId};
pub use pci::{PciAddress, PciConfig};
pub use physical::{PhysicalAccess, PhysicalError};
pub use platform::{Platform, PlatformError};
pub use power::Power;
pub use time::{CivilClock, CivilTime, Duration, Moment, Timer};
pub use user_memory::UserMemory;
