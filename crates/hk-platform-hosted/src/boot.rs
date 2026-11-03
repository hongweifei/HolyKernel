//! Hosted 平台的启动信息构造。
//!
//! 虚拟布局固定，保证确定性回放：
//!
//! | 区域 | 范围 | 类别 |
//! | --- | --- | --- |
//! | 保留 | `0x0` + 1 MiB | reserved |
//! | 设备窗口 | `0x100000` + 62 MiB | mmio |
//! | 内存 | `0x4000000` + 256 MiB | usable |
//!
//! 内核镜像位于内存区域起始处的 2 MiB；direct map 把 `0..320 MiB` 映射到
//! `0xFFFF_8000_0000_0000`。

use hk_hal::{
    Arch, BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootModule, DirectMapInfo, MemoryRegion,
    PhysAddr, RegionKind, VirtAddr,
};

/// 内核镜像起始地址，同时是内存区域起始地址。
pub const HOSTED_RAM_BASE: u64 = 64 * MIB;

/// 可用内存长度。
pub const HOSTED_RAM_SIZE: u64 = 256 * MIB;

/// direct map 的虚拟起始地址。
pub const HOSTED_DIRECT_MAP_BASE: u64 = 0xFFFF_8000_0000_0000;

const MIB: u64 = 1024 * 1024;
const RESERVED_SIZE: u64 = MIB;
const MMIO_BASE: u64 = MIB;
const MMIO_SIZE: u64 = 62 * MIB;
const KERNEL_SIZE: u64 = 2 * MIB;

static MEMORY_MAP: [MemoryRegion; 3] = [
    MemoryRegion::new(PhysAddr::new(0), RESERVED_SIZE, RegionKind::Reserved),
    MemoryRegion::new(PhysAddr::new(MMIO_BASE), MMIO_SIZE, RegionKind::Mmio),
    MemoryRegion::new(
        PhysAddr::new(HOSTED_RAM_BASE),
        HOSTED_RAM_SIZE,
        RegionKind::Usable,
    ),
];

static SERVER_MODULES: [BootModule; 0] = [];

/// 构造 Hosted 平台的启动信息。
pub fn boot_info() -> BootInfo {
    BootInfo {
        magic: BOOT_INFO_MAGIC,
        version: BOOT_INFO_VERSION,
        arch: Arch::Hosted,
        memory_map: &MEMORY_MAP,
        kernel_image: MemoryRegion::new(
            PhysAddr::new(HOSTED_RAM_BASE),
            KERNEL_SIZE,
            RegionKind::Kernel,
        ),
        framebuffer: None,
        direct_map: Some(DirectMapInfo {
            phys_base: PhysAddr::new(0),
            virt_base: VirtAddr::new(HOSTED_DIRECT_MAP_BASE),
            len: MMIO_BASE + MMIO_SIZE + HOSTED_RAM_SIZE,
        }),
        dtb: None,
        rsdp: None,
        server_modules: &SERVER_MODULES,
    }
}
