//! 用户镜像装载：原生 HKX 段、用户栈与 Arena。
//!
//! 页的分配与映射由平台的 [`UserMemory`] 提供；本模块只负责布局与镜像解析。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::image::{ImageError, load_into};
use hk_hal::{BootModule, PAGE_SIZE, UserMemory};

/// 启动时默认装载的服务；其余模块按需启动。
pub const BOOT_SERVICES: &[&str] = &[
    "hk-server-demo",
    "hk-server-echo",
    "hk-server-device",
    "hk-server-spin",
    "hk-server-virtio",
    "hk-server-holyfs",
    "hk-server-vfs",
    "hk-server-path",
    "hk-server-block",
    "hk-server-virtio-net",
    "hk-server-net",
    "hk-server-netsock",
    "hk-server-console",
    "hk-server-console-test",
    "hk-server-shell",
    "hk-server-display",
    "hk-server-display-test",
    "hk-server-terminal-test",
    "hk-server-input",
    "hk-server-pointer-test",
    "hk-server-holydoc",
    "hk-server-debug",
    "hk-server-display-restart-test",
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

/// 读取原生镜像的服务角色；缺失或无效时返回 0。
pub fn service_role(bytes: &[u8]) -> u32 {
    hk_image::Image::parse(bytes)
        .and_then(|image| image.task())
        .map_or(0, |task| task.role)
}

/// 判断原生镜像是否声明设备授权能力。
pub fn service_has_device_authority(bytes: &[u8]) -> bool {
    hk_image::Image::parse(bytes).is_some_and(|image| image.has_capability(hk_image::CAP_DEVICE))
}

/// 读取原生镜像声明的调度优先级；缺失或无效时返回默认值。
pub fn service_priority(bytes: &[u8]) -> u8 {
    hk_image::Image::parse(bytes)
        .and_then(|image| image.task())
        .map_or(16, |task| task.priority)
}

/// 镜像装载错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// 镜像结构或段映射失败。
    Image(ImageError),
    /// 物理帧耗尽或页映射失败。
    OutOfMemory,
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image(ImageError::Parse) => formatter.write_str("镜像结构无效"),
            Self::Image(ImageError::Map) => formatter.write_str("镜像段映射失败"),
            Self::OutOfMemory => formatter.write_str("物理帧耗尽或页映射失败"),
        }
    }
}

/// 镜像布局。
#[derive(Clone, Copy, Debug)]
pub struct ImageSpec<'a> {
    /// 原生镜像内容。
    pub bytes: &'a [u8],
    /// 入口首个参数：内核分配的端点编号。
    pub endpoint: u64,
    /// 用户栈基址。
    pub stack_base: u64,
    /// 用户栈大小；镜像 Task 未声明时使用。
    pub stack_size: u64,
    /// Arena 基址。
    pub arena_base: u64,
    /// Arena 大小；镜像 Task 未声明时使用。
    pub arena_size: u64,
}

/// 装载镜像、映射栈与 Arena，返回入口现场。
pub fn load<C, M, F>(spec: &ImageSpec<'_>, memory: &mut M, make_context: F) -> Result<C, LoadError>
where
    M: UserMemory,
    F: FnOnce(u64, u64, u64) -> C,
{
    let image = hk_image::Image::parse(spec.bytes).ok_or(LoadError::Image(ImageError::Parse))?;
    let task = image.task().ok_or(LoadError::Image(ImageError::Parse))?;
    let entry = load_into(spec.bytes, memory).map_err(LoadError::Image)?;
    crate::debug::add_symbols(
        image
            .symbols()
            .map(|symbol| (symbol.address, String::from(symbol.name))),
    );
    let stack_size = if task.stack_size == 0 {
        spec.stack_size
    } else {
        u64::from(task.stack_size)
    };
    let arena_size = if task.arena_size == 0 {
        spec.arena_size
    } else {
        u64::from(task.arena_size)
    };
    map_region(memory, spec.stack_base, stack_size)?;
    map_region(memory, spec.arena_base, arena_size)?;
    Ok(make_context(
        entry,
        spec.stack_base + stack_size - 8,
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

/// 取模块路径的最后一段并去掉原生镜像后缀，作为服务名。
pub fn service_name(name: &'static str) -> &'static str {
    let name = module_name(name);
    name.strip_suffix(hk_image::FILE_SUFFIX).unwrap_or(name)
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
            service_name("boot():/boot/hk-server-demo.hkx"),
            "hk-server-demo"
        );
        assert!(is_script("boot():/boot/scripts/boot.hsh"));
        assert!(!is_script("boot():/boot/hk-server-demo.hkx"));
    }

    #[test]
    fn selects_boot_services_only() {
        static MODULES: [BootModule; 3] = [
            BootModule {
                name: "boot():/boot/hk-server-demo.hkx",
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
        assert_eq!(selected[0].name, "boot():/boot/hk-server-demo.hkx");
    }
}
