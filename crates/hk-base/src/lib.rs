//! 内核、引导与 HAL 共享的基础类型。
//!
//! 只包含地址、架构标识与平台无关的启动信息，不依赖任何硬件契约；`hk-hal`
//! 在此之上定义平台 trait，`hk-boot` 在之上实现引导适配。

#![no_std]

pub mod addr;
pub mod arch;
pub mod boot;

pub use addr::{PAGE_SHIFT, PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
pub use arch::{Arch, CpuId};
pub use boot::{
    BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootInfoError, BootModule, DirectMapInfo,
    FramebufferInfo, MemoryRegion, PixelFormat, RegionKind,
};
