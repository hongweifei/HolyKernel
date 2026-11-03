//! 服务 ELF 加载。
//!
//! 从 Limine 模块中逐个取服务镜像，按程序头把各段复制到新分配的物理帧，用内核
//! 页表以用户权限映射到镜像链接地址，为每个服务分配独立用户栈与端点，登记为
//! 可运行任务后交给协作式调度器。

use alloc::format;
use alloc::string::String;
use core::cmp::min;

use hk_hal::{BootInfo, BootModule, PAGE_SIZE, VirtAddr};
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{FrameAllocator, PageFlags, PageTable};
use xmas_elf::ElfFile;
use xmas_elf::program::Type;

use crate::runtime;
use crate::service::ServiceTable;

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

    runtime::init(info.server_modules.len() + 1);
    runtime::install();

    for (index, module) in info.server_modules.iter().enumerate() {
        let name = service_name(module.name);
        let endpoint = index as u64 + 1;
        let context = load(module, index, endpoint, direct_map_base, allocator, paging)?;
        services.register(name);
        runtime::spawn(name, endpoint, context);
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

    let elf = ElfFile::new(bytes).map_err(|error| format!("解析 ELF 失败：{error}"))?;
    let entry_point = elf.header.pt2.entry_point();
    let page_size = PAGE_SIZE as u64;

    for program in elf.program_iter() {
        if program.get_type() != Ok(Type::Load) {
            continue;
        }

        let virtual_address = program.virtual_addr();
        let memory_size = program.mem_size();
        let file_size = program.file_size();
        let offset = program.offset();
        let pages = memory_size.div_ceil(page_size);

        for page in 0..pages {
            let frame = allocator
                .allocate()
                .ok_or_else(|| String::from("物理帧耗尽"))?;
            let destination = (direct_map_base + frame.get()) as *mut u8;

            // SAFETY: 新分配的帧经 HHDM 可写，长度为整页。
            unsafe {
                core::ptr::write_bytes(destination, 0, PAGE_SIZE);
            }

            let page_offset = page * page_size;
            if page_offset < file_size {
                let copy_len = min(page_size, file_size - page_offset);
                let source = bytes.as_ptr().wrapping_add((offset + page_offset) as usize);
                // SAFETY: 源落在模块镜像内，目标为新分配的整页，区间不重叠。
                unsafe {
                    core::ptr::copy_nonoverlapping(source, destination, copy_len as usize);
                }
            }

            paging
                .map(
                    VirtAddr::new(virtual_address + page_offset),
                    frame,
                    PageFlags::USER_RW,
                    allocator,
                )
                .map_err(|error| format!("映射服务段失败：{error:?}"))?;
        }
    }

    let stack_base = USER_STACK_BASE + index as u64 * USER_STACK_STRIDE;
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

fn service_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}
