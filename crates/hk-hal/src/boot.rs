//! 平台无关的启动信息。
//!
//! bootloader 适配层负责填充 `BootInfo`，内核在初始化前调用 [`BootInfo::validate`]
//! 校验内存 map、内核镜像、帧缓冲与服务器模块范围；校验失败进入早期 panic。

use core::fmt;

use crate::addr::{PAGE_SIZE, PhysAddr, VirtAddr};
use crate::arch::Arch;

/// BootInfo 魔数，取值为 ASCII `HKBOOT01`。
pub const BOOT_INFO_MAGIC: u64 = 0x484B_424F_4F54_3031;

/// 当前 BootInfo 版本。
pub const BOOT_INFO_VERSION: u32 = 1;

/// 物理内存区域类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    /// 可分配给 Untyped 的可用内存。
    Usable,
    /// 保留区域，不对内核与服务器开放。
    Reserved,
    /// 内核镜像。
    Kernel,
    /// 帧缓冲。
    Framebuffer,
    /// bootloader 提供的模块，例如服务器镜像与服务清单。
    Module,
    /// 可回收复用的 ACPI 数据。
    AcpiReclaimable,
    /// 设备 MMIO 窗口。
    Mmio,
    /// 不可用内存。
    BadMemory,
}

impl RegionKind {
    /// 返回区域类别名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Reserved => "reserved",
            Self::Kernel => "kernel",
            Self::Framebuffer => "framebuffer",
            Self::Module => "module",
            Self::AcpiReclaimable => "acpi-reclaimable",
            Self::Mmio => "mmio",
            Self::BadMemory => "bad-memory",
        }
    }

    /// 判断该区域是否可分配给内核对象与用户页。
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Usable)
    }
}

/// 一段物理内存区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    /// 起始物理地址。
    pub base: PhysAddr,
    /// 区域长度，单位为字节。
    pub len: u64,
    /// 区域类别。
    pub kind: RegionKind,
}

impl MemoryRegion {
    /// 构造内存区域。
    pub const fn new(base: PhysAddr, len: u64, kind: RegionKind) -> Self {
        Self { base, len, kind }
    }

    /// 返回结束物理地址，不含该地址。
    pub const fn end(&self) -> PhysAddr {
        PhysAddr::new(self.base.get() + self.len)
    }

    /// 判断物理地址是否落在区域内。
    pub const fn contains(&self, address: PhysAddr) -> bool {
        address.get() >= self.base.get() && address.get() < self.end().get()
    }

    /// 判断另一段区域是否完全落在本区域内。
    pub const fn contains_region(&self, other: &Self) -> bool {
        other.base.get() >= self.base.get() && other.end().get() <= self.end().get()
    }

    /// 判断两段区域是否相交。
    pub const fn overlaps(&self, other: &Self) -> bool {
        self.base.get() < other.end().get() && other.base.get() < self.end().get()
    }
}

/// 帧缓冲像素格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 32 位 RGB，每像素 4 字节。
    Rgb32,
    /// 32 位 BGR，每像素 4 字节。
    Bgr32,
}

impl PixelFormat {
    /// 返回每像素字节数。
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgb32 | Self::Bgr32 => 4,
        }
    }
}

/// 帧缓冲信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferInfo {
    /// 帧缓冲起始物理地址。
    pub phys: PhysAddr,
    /// 像素宽度。
    pub width: u32,
    /// 像素高度。
    pub height: u32,
    /// 每行字节数。
    pub stride: u32,
    /// 像素格式。
    pub format: PixelFormat,
}

/// 内核 direct map 信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectMapInfo {
    /// 映射起始物理地址。
    pub phys_base: PhysAddr,
    /// 映射起始虚拟地址。
    pub virt_base: VirtAddr,
    /// 映射长度，单位为字节。
    pub len: u64,
}

impl DirectMapInfo {
    /// 判断 direct map 是否覆盖指定物理区间。
    pub fn covers(&self, base: PhysAddr, len: u64) -> bool {
        let Some(end) = base.get().checked_add(len) else {
            return false;
        };
        let Some(map_end) = self.phys_base.get().checked_add(self.len) else {
            return false;
        };
        base.get() >= self.phys_base.get() && end <= map_end
    }

    /// 把 direct map 覆盖的物理地址转换为虚拟地址。
    pub fn virtual_address(&self, physical: PhysAddr) -> Option<VirtAddr> {
        let offset = physical.get().checked_sub(self.phys_base.get())?;
        if offset >= self.len {
            return None;
        }
        self.virt_base.get().checked_add(offset).map(VirtAddr::new)
    }
}

/// bootloader 提供的模块，例如服务器 ELF 镜像与服务清单。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootModule {
    /// 模块名称，非空。
    pub name: &'static str,
    /// 模块在物理内存中的位置。
    pub region: MemoryRegion,
}

/// 平台无关的启动信息。
#[derive(Clone, Copy, Debug)]
pub struct BootInfo {
    /// 魔数，必须等于 [`BOOT_INFO_MAGIC`]。
    pub magic: u64,
    /// 版本，必须等于 [`BOOT_INFO_VERSION`]。
    pub version: u32,
    /// 启动架构。
    pub arch: Arch,
    /// 物理内存 map，按类别描述可用与保留区域。
    pub memory_map: &'static [MemoryRegion],
    /// 内核镜像所在区域，类别为 [`RegionKind::Kernel`]。
    pub kernel_image: MemoryRegion,
    /// 帧缓冲，文本控制台平台为 `None`。
    pub framebuffer: Option<FramebufferInfo>,
    /// 内核 direct map。
    pub direct_map: Option<DirectMapInfo>,
    /// DTB 物理地址。
    pub dtb: Option<PhysAddr>,
    /// ACPI RSDP 物理地址。
    pub rsdp: Option<PhysAddr>,
    /// 服务器模块。
    pub server_modules: &'static [BootModule],
}

impl BootInfo {
    /// 校验启动信息。
    ///
    /// 校验魔数、版本、内存区域非空且互不重叠、内核镜像落在可用内存内、
    /// 帧缓冲与 direct map 参数合法、设备树与模块地址位于已描述的内存内。
    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.magic != BOOT_INFO_MAGIC {
            return Err(BootInfoError::BadMagic { found: self.magic });
        }
        if self.version != BOOT_INFO_VERSION {
            return Err(BootInfoError::UnsupportedVersion {
                found: self.version,
            });
        }
        if self.memory_map.is_empty() {
            return Err(BootInfoError::EmptyMemoryMap);
        }

        for (index, region) in self.memory_map.iter().enumerate() {
            if region.len == 0 {
                return Err(BootInfoError::EmptyRegion { index });
            }
            if region.base.get().checked_add(region.len).is_none() {
                return Err(BootInfoError::RegionOverflow { index });
            }
        }

        for (index, region) in self.memory_map.iter().enumerate() {
            for (offset, other) in self.memory_map[index + 1..].iter().enumerate() {
                if region.overlaps(other) {
                    return Err(BootInfoError::RegionOverlap {
                        index: index + 1 + offset,
                    });
                }
            }
        }

        if self.kernel_image.len == 0 {
            return Err(BootInfoError::EmptyKernelImage);
        }
        if self.kernel_image.kind != RegionKind::Kernel {
            return Err(BootInfoError::KernelKindMismatch {
                found: self.kernel_image.kind,
            });
        }
        let kernel_described = self
            .memory_map
            .iter()
            .any(|region| region.contains_region(&self.kernel_image));
        if !kernel_described {
            return Err(BootInfoError::KernelOutsideMemory {
                base: self.kernel_image.base,
            });
        }
        if let Some(direct_map) = self.direct_map
            && !direct_map.covers(self.kernel_image.base, self.kernel_image.len)
        {
            return Err(BootInfoError::DirectMapDoesNotCover { what: "kernel" });
        }

        if let Some(framebuffer) = self.framebuffer {
            let minimum_stride =
                u64::from(framebuffer.width) * u64::from(framebuffer.format.bytes_per_pixel());
            if framebuffer.width == 0
                || framebuffer.height == 0
                || u64::from(framebuffer.stride) < minimum_stride
            {
                return Err(BootInfoError::InvalidFramebuffer);
            }
            let length = u64::from(framebuffer.stride)
                .checked_mul(u64::from(framebuffer.height))
                .ok_or(BootInfoError::InvalidFramebuffer)?;
            if !self.describes_range(framebuffer.phys, length) {
                return Err(BootInfoError::AddressOutsideMemory {
                    what: "framebuffer",
                });
            }
            if let Some(direct_map) = self.direct_map
                && !direct_map.covers(framebuffer.phys, length)
            {
                return Err(BootInfoError::DirectMapDoesNotCover {
                    what: "framebuffer",
                });
            }
        }

        if let Some(direct_map) = self.direct_map {
            let aligned = direct_map.phys_base.is_page_aligned()
                && direct_map.virt_base.is_page_aligned()
                && (direct_map.len as usize).is_multiple_of(PAGE_SIZE);
            let within_address_space = direct_map
                .phys_base
                .get()
                .checked_add(direct_map.len)
                .is_some()
                && direct_map
                    .virt_base
                    .get()
                    .checked_add(direct_map.len)
                    .is_some();
            if direct_map.len == 0 || !aligned || !within_address_space {
                return Err(BootInfoError::InvalidDirectMap);
            }
        }

        if let Some(dtb) = self.dtb
            && !self.describes_address(dtb)
        {
            return Err(BootInfoError::AddressOutsideMemory { what: "dtb" });
        }
        if let Some(rsdp) = self.rsdp
            && !self.describes_address(rsdp)
        {
            return Err(BootInfoError::AddressOutsideMemory { what: "rsdp" });
        }

        for (index, module) in self.server_modules.iter().enumerate() {
            if module.name.is_empty() {
                return Err(BootInfoError::EmptyModuleName);
            }
            if module.region.len == 0 {
                return Err(BootInfoError::EmptyModule { name: module.name });
            }
            if module.region.kind != RegionKind::Module {
                return Err(BootInfoError::ModuleKindMismatch { name: module.name });
            }
            let inside = self
                .memory_map
                .iter()
                .any(|region| region.contains_region(&module.region));
            if !inside {
                return Err(BootInfoError::ModuleOutsideMemory { name: module.name });
            }
            if module.region.overlaps(&self.kernel_image) {
                return Err(BootInfoError::ModuleKernelOverlap { name: module.name });
            }
            for previous in &self.server_modules[..index] {
                if previous.name == module.name {
                    return Err(BootInfoError::DuplicateModuleName { name: module.name });
                }
                if previous.region.overlaps(&module.region) {
                    return Err(BootInfoError::ModuleOverlap {
                        first: previous.name,
                        second: module.name,
                    });
                }
            }
            if let Some(direct_map) = self.direct_map
                && !direct_map.covers(module.region.base, module.region.len)
            {
                return Err(BootInfoError::DirectMapDoesNotCover { what: "module" });
            }
        }

        Ok(())
    }

    /// 判断物理区间是否完全落在内存 map 描述的某一区域内。
    pub fn describes_range(&self, base: PhysAddr, len: u64) -> bool {
        let Some(end) = base.get().checked_add(len) else {
            return false;
        };
        self.memory_map
            .iter()
            .any(|region| base.get() >= region.base.get() && end <= region.end().get())
    }

    /// 返回启动模块的字节视图。
    ///
    /// 要求模块落在 direct map 覆盖的物理区间内；否则返回 `None`。
    pub fn module_bytes(&self, module: &BootModule) -> Option<&[u8]> {
        let direct_map = self.direct_map?;
        if !direct_map.covers(module.region.base, module.region.len) {
            return None;
        }
        let address = direct_map.virtual_address(module.region.base)?;
        let length = usize::try_from(module.region.len).ok()?;
        // SAFETY: BootInfo 的 direct map 描述承诺该物理区间已映射；长度来自模块区域。
        Some(unsafe { core::slice::from_raw_parts(address.get() as *const u8, length) })
    }

    /// 判断物理地址是否落在内存 map 描述的任一区域内。
    pub fn describes_address(&self, address: PhysAddr) -> bool {
        self.memory_map
            .iter()
            .any(|region| region.contains(address))
    }
}

impl fmt::Display for BootInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "  架构: {}", self.arch)?;
        writeln!(formatter, "  内存 map:")?;
        for region in self.memory_map {
            writeln!(
                formatter,
                "    {:#012x} + {:#010x}  {}",
                region.base.get(),
                region.len,
                region.kind.name()
            )?;
        }
        writeln!(
            formatter,
            "  内核镜像: {:#012x} + {:#010x}",
            self.kernel_image.base.get(),
            self.kernel_image.len
        )?;
        match self.framebuffer {
            Some(framebuffer) => writeln!(
                formatter,
                "  帧缓冲: {:#012x} {}x{} stride={}",
                framebuffer.phys.get(),
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride
            )?,
            None => writeln!(formatter, "  帧缓冲: 无")?,
        }
        match self.direct_map {
            Some(direct_map) => writeln!(
                formatter,
                "  direct map: {:#012x} -> {:#018x} + {:#010x}",
                direct_map.phys_base.get(),
                direct_map.virt_base.get(),
                direct_map.len
            )?,
            None => writeln!(formatter, "  direct map: 无")?,
        }
        match self.rsdp {
            Some(rsdp) => writeln!(formatter, "  RSDP: {:#012x}", rsdp.get())?,
            None => writeln!(formatter, "  RSDP: 无")?,
        }
        writeln!(formatter, "  服务器模块: {}", self.server_modules.len())
    }
}

/// BootInfo 校验错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootInfoError {
    /// 魔数不匹配。
    BadMagic {
        /// 实际读到的魔数。
        found: u64,
    },
    /// 版本不受支持。
    UnsupportedVersion {
        /// 实际读到的版本。
        found: u32,
    },
    /// 内存 map 为空。
    EmptyMemoryMap,
    /// 内存区域长度为 0。
    EmptyRegion {
        /// 区域下标。
        index: usize,
    },
    /// 内存区域末尾溢出。
    RegionOverflow {
        /// 区域下标。
        index: usize,
    },
    /// 内存区域相互重叠。
    RegionOverlap {
        /// 与前序区域重叠的区域下标。
        index: usize,
    },
    /// 内核镜像区域类别不是 [`RegionKind::Kernel`]。
    KernelKindMismatch {
        /// 实际类别。
        found: RegionKind,
    },
    /// 内核镜像未落在内存 map 描述的任何区域内。
    KernelOutsideMemory {
        /// 内核镜像起始地址。
        base: PhysAddr,
    },
    /// 帧缓冲参数非法。
    InvalidFramebuffer,
    /// direct map 参数非法。
    InvalidDirectMap,
    /// 地址未落在已描述的内存内。
    AddressOutsideMemory {
        /// 地址来源，取值为 `dtb` 或 `rsdp`。
        what: &'static str,
    },
    /// 模块类别不是 [`RegionKind::Module`]。
    ModuleKindMismatch {
        /// 模块名称。
        name: &'static str,
    },
    /// 模块未落在已描述的内存内。
    ModuleOutsideMemory {
        /// 模块名称。
        name: &'static str,
    },
    /// 模块名称为空。
    EmptyModuleName,
    /// 内核镜像为空。
    EmptyKernelImage,
    /// 模块区域为空。
    EmptyModule {
        /// 模块名称。
        name: &'static str,
    },
    /// direct map 未覆盖必要区域。
    DirectMapDoesNotCover {
        /// 区域来源，取值为 `kernel`、`framebuffer` 或 `module`。
        what: &'static str,
    },
    /// 模块与内核镜像重叠。
    ModuleKernelOverlap {
        /// 模块名称。
        name: &'static str,
    },
    /// 两个模块区域重叠。
    ModuleOverlap {
        /// 前一个模块。
        first: &'static str,
        /// 后一个模块。
        second: &'static str,
    },
    /// 模块名称重复。
    DuplicateModuleName {
        /// 重复的名称。
        name: &'static str,
    },
}

impl fmt::Display for BootInfoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic { found } => write!(formatter, "BootInfo 魔数不匹配：{found:#018x}"),
            Self::UnsupportedVersion { found } => {
                write!(formatter, "BootInfo 版本不受支持：{found}")
            }
            Self::EmptyMemoryMap => formatter.write_str("BootInfo 内存 map 为空"),
            Self::EmptyRegion { index } => write!(formatter, "内存区域 {index} 长度为 0"),
            Self::RegionOverflow { index } => write!(formatter, "内存区域 {index} 末尾溢出"),
            Self::RegionOverlap { index } => write!(formatter, "内存区域 {index} 与前序区域重叠"),
            Self::KernelKindMismatch { found } => {
                write!(
                    formatter,
                    "内核镜像区域类别为 {}，期望 kernel",
                    found.name()
                )
            }
            Self::KernelOutsideMemory { base } => {
                write!(formatter, "内核镜像 {base:?} 未落在已描述的内存内")
            }
            Self::InvalidFramebuffer => formatter.write_str("帧缓冲参数非法"),
            Self::InvalidDirectMap => formatter.write_str("direct map 参数非法"),
            Self::AddressOutsideMemory { what } => {
                write!(formatter, "{what} 地址未落在已描述的内存内")
            }
            Self::ModuleKindMismatch { name } => {
                write!(formatter, "模块 {name} 的区域类别不是 module")
            }
            Self::ModuleOutsideMemory { name } => {
                write!(formatter, "模块 {name} 未落在已描述的内存内")
            }
            Self::EmptyModuleName => formatter.write_str("模块名称为空"),
            Self::EmptyKernelImage => formatter.write_str("内核镜像为空"),
            Self::EmptyModule { name } => write!(formatter, "模块 {name} 的区域为空"),
            Self::DirectMapDoesNotCover { what } => {
                write!(formatter, "direct map 未覆盖 {what}")
            }
            Self::ModuleKernelOverlap { name } => {
                write!(formatter, "模块 {name} 与内核镜像重叠")
            }
            Self::ModuleOverlap { first, second } => {
                write!(formatter, "模块 {first} 与模块 {second} 重叠")
            }
            Self::DuplicateModuleName { name } => write!(formatter, "模块名称重复：{name}"),
        }
    }
}

impl core::error::Error for BootInfoError {}

#[cfg(test)]
mod tests {
    use super::*;

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
}
