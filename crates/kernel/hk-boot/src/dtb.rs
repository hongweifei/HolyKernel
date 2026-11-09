//! 设备树（DTB）引导后端。
//!
//! 适用于 QEMU `-kernel` 等由固件直接提供 DTB 的启动方式：内存 map 来自
//! `/memory` 与 `/reserved-memory`，内核镜像、direct map、帧缓冲与模块由平台
//! 提供。

use alloc::boxed::Box;
use alloc::vec::Vec;

use hk_base::{
    Arch, BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo, BootModule, DirectMapInfo, FramebufferInfo,
    MemoryRegion, PhysAddr, RegionKind,
};
use hk_fw::MemoryKind;
use hk_fw::dtb::DtbInfo;

use crate::{BootBackend, BootError};

/// 由设备树驱动的引导配置。
#[derive(Clone, Debug)]
pub struct DtbBoot {
    /// 已解析的设备树。
    pub dtb: DtbInfo,
    /// DTB 物理地址。
    pub dtb_phys: PhysAddr,
    /// 架构标识。
    pub arch: Arch,
    /// 内核镜像区域，类别应为 [`RegionKind::Kernel`]。
    pub kernel_image: MemoryRegion,
    /// direct map 描述。
    pub direct_map: Option<DirectMapInfo>,
    /// 帧缓冲描述。
    pub framebuffer: Option<FramebufferInfo>,
    /// 随启动加载的服务模块。
    pub modules: &'static [BootModule],
    /// 内核命令行。
    pub cmdline: Option<&'static str>,
}

impl DtbBoot {
    /// 把 DTB 内存描述转换为 BootInfo 内存 map。
    fn memory_map(&self) -> Vec<MemoryRegion> {
        self.dtb
            .memory
            .iter()
            .map(|region| {
                MemoryRegion::new(
                    PhysAddr::new(region.base),
                    region.len,
                    match region.kind {
                        MemoryKind::Usable => RegionKind::Usable,
                        MemoryKind::Reserved => RegionKind::Reserved,
                        MemoryKind::AcpiReclaimable => RegionKind::AcpiReclaimable,
                        MemoryKind::Device => RegionKind::Mmio,
                    },
                )
            })
            .collect()
    }
}

impl BootBackend for DtbBoot {
    fn boot_info(&self) -> Result<BootInfo, BootError> {
        let memory_map: &'static [MemoryRegion] = Box::leak(self.memory_map().into_boxed_slice());
        Ok(BootInfo {
            magic: BOOT_INFO_MAGIC,
            version: BOOT_INFO_VERSION,
            arch: self.arch,
            memory_map,
            kernel_image: self.kernel_image,
            framebuffer: self.framebuffer,
            direct_map: self.direct_map,
            dtb: Some(self.dtb_phys),
            rsdp: None,
            server_modules: self.modules,
        })
    }

    fn cmdline(&self) -> Option<&'static str> {
        self.cmdline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use hk_base::VirtAddr;
    use hk_fw::MemoryRegion as FwMemoryRegion;

    fn boot() -> DtbBoot {
        DtbBoot {
            dtb: DtbInfo {
                memory: vec![
                    FwMemoryRegion {
                        base: 0x4000_0000,
                        len: 0x1000_0000,
                        kind: MemoryKind::Usable,
                    },
                    FwMemoryRegion {
                        base: 0,
                        len: 0x1000,
                        kind: MemoryKind::Reserved,
                    },
                ],
                cpus: Vec::new(),
                devices: Vec::new(),
                boot_cpu: 0,
            },
            dtb_phys: PhysAddr::new(0x4000_0000),
            arch: Arch::Aarch64,
            kernel_image: MemoryRegion::new(
                PhysAddr::new(0x4008_0000),
                0x10_0000,
                RegionKind::Kernel,
            ),
            direct_map: Some(DirectMapInfo {
                phys_base: PhysAddr::new(0),
                virt_base: VirtAddr::new(0xFFFF_8000_0000_0000),
                len: 0x5000_0000,
            }),
            framebuffer: None,
            modules: &[],
            cmdline: Some("console=ttyAMA0"),
        }
    }

    #[test]
    fn maps_dtb_memory_and_validates() {
        let boot = boot();
        let info = boot.boot_info().expect("构造启动信息");
        assert_eq!(info.arch, Arch::Aarch64);
        assert_eq!(info.dtb, Some(PhysAddr::new(0x4000_0000)));
        assert_eq!(info.memory_map.len(), 2);
        assert_eq!(info.memory_map[0].kind, RegionKind::Usable);
        assert_eq!(info.memory_map[1].kind, RegionKind::Reserved);
        info.validate().expect("启动信息合法");
        assert_eq!(boot.cmdline(), Some("console=ttyAMA0"));
    }
}
