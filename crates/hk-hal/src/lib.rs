//! HolyKernel 硬件抽象层。
//!
//! 内核只依赖本 crate 的 trait 访问平台；架构差异位于 `hk-platform-*` 实现。
//! HAL 与内核接口使用 `no_std` 语义；Hosted 平台在实现内部使用 `std`。

#![no_std]

#[cfg(test)]
extern crate std;

pub mod addr;
pub mod arch;
pub mod boot;
pub mod console;
pub mod dma;
pub mod entropy;
pub mod execution;
pub mod irq;
pub mod mmu;
pub mod pci;
pub mod physical;
pub mod platform;
pub mod power;
pub mod time;

pub use addr::{PAGE_SHIFT, PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
pub use arch::{Arch, CpuId};
pub use boot::{
    BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootInfoError, BootModule, DirectMapInfo,
    FramebufferInfo, MemoryRegion, PixelFormat, RegionKind,
};
pub use console::Console;
pub use dma::{DmaBuffer, DmaError, DmaOps};
pub use entropy::Entropy;
pub use execution::UserContext;
pub use irq::{InterruptController, Ipi, Irq, IrqError, NotificationId};
pub use mmu::{MapFlags, Mmu, MmuError, VSpaceId};
pub use pci::{PciAddress, PciConfig};
pub use physical::{PhysicalAccess, PhysicalError};
pub use platform::{Platform, PlatformError};
pub use power::Power;
pub use time::{Duration, Moment, Timer};
