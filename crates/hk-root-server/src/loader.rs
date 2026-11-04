//! 用户镜像装载：ELF 段、用户栈与 Arena。
//!
//! 页的分配与映射由平台的 [`UserMemory`] 提供；本模块只负责布局与镜像解析。

use alloc::vec::Vec;
use core::fmt;

use crate::elf::{ElfError, load_into};
use hk_hal::{BootModule, PAGE_SIZE, UserMemory};
use hk_syscall::ServiceRole;
use xmas_elf::ElfFile;

/// 启动时默认装载的服务；其余模块按需启动。
pub const BOOT_SERVICES: &[&str] = &[
    "hk-server-demo",
    "hk-server-echo",
    "hk-server-device",
    "hk-server-spin",
    "hk-server-vfs",
    "hk-server-block",
];

/// 从启动模块中选出默认启动的服务。
pub fn boot_modules(modules: &[BootModule]) -> Vec<&BootModule> {
    modules
        .iter()
        .filter(|module| {
            !is_script(module.name) && BOOT_SERVICES.contains(&service_name(module.name))
        })
        .collect()
}

/// 读取镜像的 `.hk_service` 角色元数据；缺失或无效时返回 0。
pub fn service_role(bytes: &[u8]) -> u32 {
    let Ok(file) = ElfFile::new(bytes) else {
        return 0;
    };
    for section in file.section_iter() {
        if section.get_name(&file).ok() != Some(".hk_service") {
            continue;
        }
        let data = section.raw_data(&file);
        let (magic, role) = match data {
            [a, b, c, d, e, f, g, h, ..] => (
                u32::from_le_bytes([*a, *b, *c, *d]),
                u32::from_le_bytes([*e, *f, *g, *h]),
            ),
            _ => return 0,
        };
        if magic == ServiceRole::MAGIC {
            return role;
        }
    }
    0
}

/// 镜像装载错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// ELF 结构或段映射失败。
    Elf(ElfError),
    /// 物理帧耗尽或页映射失败。
    OutOfMemory,
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Elf(ElfError::Parse) => formatter.write_str("ELF 结构无效"),
            Self::Elf(ElfError::Map) => formatter.write_str("ELF 段映射失败"),
            Self::OutOfMemory => formatter.write_str("物理帧耗尽或页映射失败"),
        }
    }
}

/// 镜像布局。
#[derive(Clone, Copy, Debug)]
pub struct ImageSpec<'a> {
    /// ELF 文件内容。
    pub bytes: &'a [u8],
    /// 入口首个参数：内核分配的端点编号。
    pub endpoint: u64,
    /// 用户栈基址。
    pub stack_base: u64,
    /// 用户栈大小。
    pub stack_size: u64,
    /// Arena 基址。
    pub arena_base: u64,
    /// Arena 大小。
    pub arena_size: u64,
}

/// 装载镜像、映射栈与 Arena，返回入口现场。
pub fn load<C, M, F>(spec: &ImageSpec<'_>, memory: &mut M, make_context: F) -> Result<C, LoadError>
where
    M: UserMemory,
    F: FnOnce(u64, u64, u64) -> C,
{
    let entry = load_into(spec.bytes, memory).map_err(LoadError::Elf)?;
    map_region(memory, spec.stack_base, spec.stack_size)?;
    map_region(memory, spec.arena_base, spec.arena_size)?;
    Ok(make_context(
        entry,
        spec.stack_base + spec.stack_size - 8,
        spec.endpoint,
    ))
}

fn map_region<M: UserMemory>(memory: &mut M, base: u64, size: u64) -> Result<(), LoadError> {
    let page_size = PAGE_SIZE as u64;
    let pages = size.div_ceil(page_size);
    for page in 0..pages {
        memory
            .map_page(base + page * page_size)
            .ok_or(LoadError::OutOfMemory)?;
    }
    Ok(())
}

/// 取模块路径的最后一段。
pub fn module_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// 取模块路径的最后一段作为服务名。
pub fn service_name(name: &'static str) -> &'static str {
    module_name(name)
}

/// 判断模块是否为 HolyShell 脚本。
pub fn is_script(name: &str) -> bool {
    module_name(name).ends_with(".hsh")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::{MemoryRegion, PhysAddr, RegionKind};

    #[test]
    fn extracts_module_names() {
        assert_eq!(module_name("boot():/boot/scripts/boot.hsh"), "boot.hsh");
        assert_eq!(
            service_name("boot():/boot/hk-server-demo"),
            "hk-server-demo"
        );
        assert!(is_script("boot():/boot/scripts/boot.hsh"));
        assert!(!is_script("boot():/boot/hk-server-demo"));
    }

    #[test]
    fn selects_boot_services_only() {
        static MODULES: [BootModule; 3] = [
            BootModule {
                name: "boot():/boot/hk-server-demo",
                region: MemoryRegion::new(PhysAddr::new(0x1000), 0x1000, RegionKind::Module),
            },
            BootModule {
                name: "boot():/boot/scripts/boot.hsh",
                region: MemoryRegion::new(PhysAddr::new(0x2000), 0x1000, RegionKind::Module),
            },
            BootModule {
                name: "boot():/boot/hk-server-fault",
                region: MemoryRegion::new(PhysAddr::new(0x3000), 0x1000, RegionKind::Module),
            },
        ];

        let selected = boot_modules(&MODULES);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "boot():/boot/hk-server-demo");
    }
}
