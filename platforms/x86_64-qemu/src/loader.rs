//! 服务 ELF 加载与任务登记。
//!
//! 从 Limine 模块中逐个取服务镜像，用 `hk-elf` 遍历可加载段，平台实现段页映射；
//! 每个服务获得独立用户栈与端点，登记为任务后交给 `hk-kernel` 调度。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::min;

use hk_elf::{MapError, PageSink};
use hk_hal::{BootInfo, BootModule, PAGE_SIZE, VirtAddr};
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{FrameAllocator, PageFlags, PageTable};
use hk_root_server::ServiceTable;

use crate::runtime;

/// 用户栈基址。
const USER_STACK_BASE: u64 = 0x0080_0000;

/// 用户栈大小。
const USER_STACK_SIZE: u64 = 8 * PAGE_SIZE as u64;

/// 各服务用户栈之间的间隔。
const USER_STACK_STRIDE: u64 = 0x1_0000;

/// 加载全部服务模块并运行到退出。
pub fn run(
    info: &BootInfo,
    direct_map_base: u64,
    allocator: &mut FrameAllocator,
    paging: &PageTable,
    services: &mut ServiceTable,
) -> Result<(), String> {
    if info.server_modules.is_empty() {
        return Err(String::from("没有服务模块"));
    }

    let names: Vec<&'static str> = info
        .server_modules
        .iter()
        .map(|module| service_name(module.name))
        .collect();
    runtime::init(&names)?;

    for (index, module) in info.server_modules.iter().enumerate() {
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

    let entry_point = {
        let mut pages = UserPages {
            direct_map_base,
            paging,
            allocator,
        };
        hk_elf::load(bytes, &mut pages).map_err(|error| format!("装载服务 ELF 失败：{error:?}"))?
    };

    let stack_base = USER_STACK_BASE + index as u64 * USER_STACK_STRIDE;
    let page_size = PAGE_SIZE as u64;
    let stack_pages = USER_STACK_SIZE / page_size;
    for page in 0..stack_pages {
        let frame = allocator
            .allocate()
            .ok_or_else(|| String::from("物理帧耗尽"))?;
        let destination = (direct_map_base + frame.get()) as *mut u8;
        // SAFETY: 新分配的帧经 HHDM 可写，长度为整页。
        unsafe {
            core::ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        paging
            .map(
                VirtAddr::new(stack_base + page * page_size),
                frame,
                PageFlags::USER_RW,
                allocator,
            )
            .map_err(|error| format!("映射用户栈失败：{error:?}"))?;
    }

    Ok(cpu::user_frame(
        entry_point,
        stack_base + USER_STACK_SIZE,
        endpoint,
    ))
}

struct UserPages<'a> {
    direct_map_base: u64,
    paging: &'a PageTable,
    allocator: &'a mut FrameAllocator,
}

impl PageSink for UserPages<'_> {
    fn map_segment(
        &mut self,
        virtual_address: u64,
        file: &[u8],
        memory_size: u64,
    ) -> Result<(), MapError> {
        let page_size = PAGE_SIZE as u64;
        let pages = memory_size.div_ceil(page_size);

        for page in 0..pages {
            let frame = self.allocator.allocate().ok_or(MapError)?;
            let destination = (self.direct_map_base + frame.get()) as *mut u8;

            // SAFETY: 新分配的帧经 HHDM 可写，长度为整页。
            unsafe {
                core::ptr::write_bytes(destination, 0, PAGE_SIZE);
            }

            let page_offset = page * page_size;
            if page_offset < file.len() as u64 {
                let copy_len = min(page_size, file.len() as u64 - page_offset) as usize;
                let source = file.as_ptr().wrapping_add(page_offset as usize);
                // SAFETY: 源落在段文件内容内，目标为新分配的整页，区间不重叠。
                unsafe {
                    core::ptr::copy_nonoverlapping(source, destination, copy_len);
                }
            }

            self.paging
                .map(
                    VirtAddr::new(virtual_address + page_offset),
                    frame,
                    PageFlags::USER_RW,
                    self.allocator,
                )
                .map_err(|_| MapError)?;
        }
        Ok(())
    }
}

fn service_name(name: &'static str) -> &'static str {
    name.rsplit('/').next().unwrap_or(name)
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
        .find(|module| service_name(module.name) == name)
        .ok_or_else(|| format!("未知服务：{name}"))?;
    load(module, slot, 0, direct_map_base, allocator, paging)
}
