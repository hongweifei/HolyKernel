//! 服务 ELF 加载与任务登记。
//!
//! 镜像解析、用户栈与 Arena 布局由 `hk-root-server::loader` 完成；本模块提供
//! x86 用户页映射（`UserMemory`）与启动信息到镜像布局的适配。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{BootInfo, BootModule, PAGE_SIZE, UserMemory, VirtAddr};
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{FrameAllocator, PageFlags, PageTable};
use hk_root_server::ServiceTable;
use hk_root_server::loader::{self, ImageSpec};

use crate::runtime;

/// 用户栈基址。
const USER_STACK_BASE: u64 = 0x0080_0000;

/// 用户栈大小。
const USER_STACK_SIZE: u64 = 8 * PAGE_SIZE as u64;

/// 各服务用户栈之间的间隔。
const USER_STACK_STRIDE: u64 = 0x1_0000;

/// 启动时装载的服务；其余模块按需启动。
const BOOT_SERVICES: &[&str] = &[
    "hk-server-demo",
    "hk-server-echo",
    "hk-server-device",
    "hk-server-spin",
];

/// 加载全部服务模块并运行到退出。
pub fn run(
    info: &BootInfo,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
    services: &mut ServiceTable,
) -> Result<(), String> {
    let modules: Vec<&BootModule> = info
        .server_modules
        .iter()
        .filter(|module| {
            !loader::is_script(module.name)
                && BOOT_SERVICES.contains(&loader::service_name(module.name))
        })
        .collect();
    if modules.is_empty() {
        return Err(String::from("没有服务模块"));
    }

    let names: Vec<&'static str> = modules
        .iter()
        .map(|module| loader::service_name(module.name))
        .collect();
    runtime::init(&names)?;

    for (index, module) in modules.iter().enumerate() {
        let endpoint = index as u64 + 1;
        let context = load(module, index, endpoint, direct_map_base, allocator, paging)?;
        services.register(names[index]);
        runtime::attach(index, context);
    }

    runtime::run(services);
    Ok(())
}

fn load(
    module: &BootModule,
    index: usize,
    endpoint: u64,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
) -> Result<cpu::InterruptFrame, String> {
    let module_virtual = direct_map_base + module.region.base.get();
    // SAFETY: 模块区域由 bootloader 载入并落在 HHDM 映射内，长度来自启动信息。
    let bytes = unsafe {
        core::slice::from_raw_parts(module_virtual as *const u8, module.region.len as usize)
    };

    let stack_base = USER_STACK_BASE + index as u64 * USER_STACK_STRIDE;
    let (arena_base, arena_size) = runtime::arena(index);
    let spec = ImageSpec {
        bytes,
        endpoint,
        stack_base,
        stack_size: USER_STACK_SIZE,
        arena_base,
        arena_size,
    };

    let table = paging
        .create_user(allocator)
        .map_err(|error| format!("创建地址空间失败：{error:?}"))?;
    let mut memory = UserPages {
        direct_map_base,
        paging: &table,
        allocator,
    };
    let context = loader::load(&spec, &mut memory, |entry, stack_top, endpoint| {
        cpu::user_frame(entry, stack_top, endpoint)
    })
    .map_err(|error| format!("装载服务失败：{error}"))?;
    drop(memory);
    runtime::install_vspace(index, table);
    Ok(context)
}

struct UserPages<'a> {
    direct_map_base: u64,
    paging: &'a PageTable,
    allocator: &'a mut FrameAllocator,
}

impl UserMemory for UserPages<'_> {
    fn map_page(&mut self, virtual_address: u64) -> Option<&mut [u8]> {
        let frame = self.allocator.allocate()?;
        let destination = (self.direct_map_base + frame.get()) as *mut u8;

        // SAFETY: 新分配的帧经 HHDM 可写，长度为整页。
        unsafe {
            core::ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        self.paging
            .map(
                VirtAddr::new(virtual_address),
                frame,
                PageFlags::USER_RW,
                self.allocator,
            )
            .ok()?;

        // SAFETY: 帧经 HHDM 映射为整页可写；映射建立后返回该页视图。
        Some(unsafe { core::slice::from_raw_parts_mut(destination, PAGE_SIZE) })
    }
}

/// 按名称装载服务模块，用于按需启动。
pub fn load_service(
    info: &BootInfo,
    name: &str,
    slot: usize,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
) -> Result<cpu::InterruptFrame, String> {
    let module = info
        .server_modules
        .iter()
        .find(|module| !loader::is_script(module.name) && loader::service_name(module.name) == name)
        .ok_or_else(|| format!("未知服务：{name}"))?;
    load(module, slot, 0, direct_map_base, allocator, paging)
}
