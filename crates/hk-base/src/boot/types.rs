//! 启动信息类型与访问器。

use crate::addr::{PhysAddr, VirtAddr};
use crate::arch::Arch;

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

    /// 返回跨内核与服务共享的格式编码。
    pub const fn code(self) -> u32 {
        match self {
            Self::Rgb32 => 0,
            Self::Bgr32 => 1,
        }
    }

    /// 由格式编码还原；未知编码返回 `None`。
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Rgb32),
            1 => Some(Self::Bgr32),
            _ => None,
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
