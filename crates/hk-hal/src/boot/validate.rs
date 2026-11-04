//! 启动信息校验。

use core::fmt;

use super::*;
use crate::addr::{PAGE_SIZE, PhysAddr};

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
