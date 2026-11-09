//! 动态 per-CPU 系统调用栈与 guard page。
//!
//! 每个 CPU 在独立内核窗口中获得 32 KiB 栈；窗口首个页保持未映射，作为栈
//! 向下溢出的 guard page。初始化失败时调用方保留静态栈。

use hk_hal::{MAX_CPUS, PAGE_SIZE, PhysAddr, VirtAddr};
use hk_mem::FrameKind;

use crate::paging::{self, PageAllocator, PageFlags, PageTable};

/// 动态栈窗口基址。
const WINDOW_BASE: u64 = 0xFFFF_B000_0000_0000;

/// 每个 CPU 的栈页数（32 KiB）。
const STACK_PAGES: u64 = 8;

const PAGE: u64 = PAGE_SIZE as u64;

/// 每个 CPU 的跨越长度：一个 guard 页 + 栈页。
const STRIDE: u64 = (STACK_PAGES + 1) * PAGE;

/// 为每个 CPU 分配栈并保留 guard page；成功后更新 TSS.rsp0。
pub fn init(paging: &PageTable, allocator: &PageAllocator) -> bool {
    let direct_map = paging::direct_map_base();
    if direct_map == 0 {
        return false;
    }
    for cpu in 0..MAX_CPUS {
        // 每段的首个页不建立映射，作为 guard page。
        let stack_base = WINDOW_BASE + cpu as u64 * STRIDE + PAGE;
        for page in 0..STACK_PAGES {
            let Some(phys) = allocator.allocate_for(FrameKind::Other) else {
                return false;
            };
            zero_frame(direct_map, phys);
            if paging
                .map(
                    VirtAddr::new(stack_base + page * PAGE),
                    phys,
                    PageFlags::KERNEL_STACK,
                    allocator,
                )
                .is_err()
            {
                return false;
            }
        }
        // guard 页必须保持未映射，栈页必须已映射。
        let guard = WINDOW_BASE + cpu as u64 * STRIDE;
        if paging.translate(VirtAddr::new(guard)).is_some()
            || paging.translate(VirtAddr::new(stack_base)).is_none()
        {
            return false;
        }
        // 预留中断帧与 FPU 现场，并保持 FPU 区域 64 字节对齐。
        let top = stack_base + STACK_PAGES * PAGE - super::frame::STACK_RESERVE as u64;
        crate::cpu::set_syscall_stack_top(cpu, top);
    }
    true
}

fn zero_frame(direct_map: u64, phys: PhysAddr) {
    let pointer = (direct_map + phys.get()) as *mut u8;
    // SAFETY: 新分配的物理帧经 direct map 可写，长度为整页。
    unsafe {
        core::ptr::write_bytes(pointer, 0, PAGE_SIZE);
    }
}
