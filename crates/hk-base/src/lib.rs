//! 内核、引导与 HAL 共享的基础类型。
//!
//! 包含地址、架构标识、启动信息、时间与用户地址布局等内核与用户态共用的类型，
//! 不依赖任何硬件契约；`hk-hal` 在此之上定义平台 trait，`hk-boot` 在之上实现
//! 引导适配。

#![no_std]

pub mod addr;
pub mod arch;
pub mod boot;
pub mod irq;
pub mod time;
pub mod user;

pub use addr::{PAGE_SHIFT, PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
pub use arch::{Arch, CpuId};
pub use boot::{
    BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootInfoError, BootModule, DirectMapInfo,
    FramebufferInfo, MemoryRegion, PixelFormat, RegionKind,
};
pub use irq::IRQ_VECTOR_BASE;
pub use time::{CivilClock, CivilTime, Duration, Moment};
