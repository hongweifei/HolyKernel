//! 启动信息校验测试。

use super::*;

use crate::addr::{PhysAddr, VirtAddr};
use crate::arch::Arch;

static MEMORY: [MemoryRegion; 2] = [
    MemoryRegion::new(PhysAddr::new(0), 0x1000, RegionKind::Reserved),
    MemoryRegion::new(PhysAddr::new(0x1000), 0x1_0000, RegionKind::Usable),
];

static OVERLAPPING: [MemoryRegion; 2] = [
    MemoryRegion::new(PhysAddr::new(0), 0x2000, RegionKind::Reserved),
    MemoryRegion::new(PhysAddr::new(0x1000), 0x2000, RegionKind::Usable),
];

static MODULE: [BootModule; 1] = [BootModule {
    name: "root-server",
    region: MemoryRegion::new(PhysAddr::new(0x8000), 0x1000, RegionKind::Module),
}];

static OUTSIDE_MODULE: [BootModule; 1] = [BootModule {
    name: "root-server",
    region: MemoryRegion::new(PhysAddr::new(0x10_0000), 0x1000, RegionKind::Module),
}];

static KERNEL_OVERLAP_MODULE: [BootModule; 1] = [BootModule {
    name: "overlap",
    region: MemoryRegion::new(PhysAddr::new(0x2000), 0x1000, RegionKind::Module),
}];

static OVERLAP_MODULES: [BootModule; 2] = [
    BootModule {
        name: "a",
        region: MemoryRegion::new(PhysAddr::new(0x8000), 0x1000, RegionKind::Module),
    },
    BootModule {
        name: "b",
        region: MemoryRegion::new(PhysAddr::new(0x8000), 0x1000, RegionKind::Module),
    },
];

static DUPLICATE_MODULES: [BootModule; 2] = [
    BootModule {
        name: "a",
        region: MemoryRegion::new(PhysAddr::new(0x8000), 0x1000, RegionKind::Module),
    },
    BootModule {
        name: "a",
        region: MemoryRegion::new(PhysAddr::new(0x9000), 0x1000, RegionKind::Module),
    },
];

fn boot_info() -> BootInfo {
    BootInfo {
        magic: BOOT_INFO_MAGIC,
        version: BOOT_INFO_VERSION,
        arch: Arch::Hosted,
        memory_map: &MEMORY,
        kernel_image: MemoryRegion::new(PhysAddr::new(0x2000), 0x1000, RegionKind::Kernel),
        framebuffer: None,
        direct_map: Some(DirectMapInfo {
            phys_base: PhysAddr::new(0),
            virt_base: VirtAddr::new(0xFFFF_8000_0000_0000),
            len: 0x1_0000,
        }),
        dtb: None,
        rsdp: None,
        server_modules: &[],
    }
}

#[test]
fn accepts_valid_boot_info() {
    assert_eq!(boot_info().validate(), Ok(()));
}

#[test]
fn rejects_wrong_magic() {
    let mut boot = boot_info();
    boot.magic = 0;
    assert_eq!(boot.validate(), Err(BootInfoError::BadMagic { found: 0 }));
}

#[test]
fn rejects_unsupported_version() {
    let mut boot = boot_info();
    boot.version = 99;
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::UnsupportedVersion { found: 99 })
    );
}

#[test]
fn rejects_overlapping_regions() {
    let mut boot = boot_info();
    boot.memory_map = &OVERLAPPING;
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::RegionOverlap { index: 1 })
    );
}

#[test]
fn rejects_kernel_outside_memory() {
    let mut boot = boot_info();
    boot.kernel_image = MemoryRegion::new(PhysAddr::new(0x20_0000), 0x1000, RegionKind::Kernel);
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::KernelOutsideMemory {
            base: PhysAddr::new(0x20_0000)
        })
    );
}

#[test]
fn rejects_invalid_framebuffer_stride() {
    let mut boot = boot_info();
    boot.framebuffer = Some(FramebufferInfo {
        phys: PhysAddr::new(0x4000),
        width: 640,
        height: 480,
        stride: 1280,
        format: PixelFormat::Rgb32,
    });
    assert_eq!(boot.validate(), Err(BootInfoError::InvalidFramebuffer));
}

#[test]
fn rejects_module_outside_memory() {
    let mut boot = boot_info();
    boot.server_modules = &OUTSIDE_MODULE;
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::ModuleOutsideMemory {
            name: "root-server"
        })
    );
}

#[test]
fn accepts_module_inside_memory() {
    let mut boot = boot_info();
    boot.server_modules = &MODULE;
    assert_eq!(boot.validate(), Ok(()));
}

#[test]
fn rejects_address_outside_memory() {
    let mut boot = boot_info();
    boot.dtb = Some(PhysAddr::new(0x40_0000));
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::AddressOutsideMemory { what: "dtb" })
    );
}

#[test]
fn rejects_empty_kernel_image() {
    let mut boot = boot_info();
    boot.kernel_image = MemoryRegion::new(PhysAddr::new(0x2000), 0, RegionKind::Kernel);
    assert_eq!(boot.validate(), Err(BootInfoError::EmptyKernelImage));
}

#[test]
fn rejects_direct_map_without_kernel() {
    let mut boot = boot_info();
    boot.direct_map = Some(DirectMapInfo {
        phys_base: PhysAddr::new(0),
        virt_base: VirtAddr::new(0xFFFF_8000_0000_0000),
        len: 0x1000,
    });
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::DirectMapDoesNotCover { what: "kernel" })
    );
}

#[test]
fn rejects_framebuffer_outside_memory() {
    let mut boot = boot_info();
    boot.framebuffer = Some(FramebufferInfo {
        phys: PhysAddr::new(0x20_0000),
        width: 10,
        height: 10,
        stride: 64,
        format: PixelFormat::Rgb32,
    });
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::AddressOutsideMemory {
            what: "framebuffer"
        })
    );
}

#[test]
fn accepts_framebuffer_inside_direct_map() {
    let mut boot = boot_info();
    boot.framebuffer = Some(FramebufferInfo {
        phys: PhysAddr::new(0x4000),
        width: 10,
        height: 10,
        stride: 64,
        format: PixelFormat::Rgb32,
    });
    assert_eq!(boot.validate(), Ok(()));
}

#[test]
fn rejects_module_kernel_overlap() {
    let mut boot = boot_info();
    boot.server_modules = &KERNEL_OVERLAP_MODULE;
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::ModuleKernelOverlap { name: "overlap" })
    );
}

#[test]
fn rejects_module_overlap_and_duplicate_names() {
    let mut boot = boot_info();
    boot.server_modules = &OVERLAP_MODULES;
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::ModuleOverlap {
            first: "a",
            second: "b"
        })
    );

    boot.server_modules = &DUPLICATE_MODULES;
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::DuplicateModuleName { name: "a" })
    );
}

#[test]
fn rejects_direct_map_without_module() {
    let mut boot = boot_info();
    boot.server_modules = &MODULE;
    boot.direct_map = Some(DirectMapInfo {
        phys_base: PhysAddr::new(0),
        virt_base: VirtAddr::new(0xFFFF_8000_0000_0000),
        len: 0x4000,
    });
    assert_eq!(
        boot.validate(),
        Err(BootInfoError::DirectMapDoesNotCover { what: "module" })
    );
}
