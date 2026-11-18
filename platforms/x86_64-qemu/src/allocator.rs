//! 内核堆：启动期 bootstrap 小堆 + 可按需增长的 FrameBank 主堆。

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};
use hk_kernel::KernelHeap;
use hk_mem::FrameKind;

/// 启动期 bootstrap 堆大小。
const BOOTSTRAP_SIZE: usize = 1024 * 1024;

#[global_allocator]
pub(crate) static ALLOCATOR: KernelHeap<BOOTSTRAP_SIZE> = KernelHeap::new();

/// 以 FrameBank 供给的虚拟区间初始化内核主堆。
pub(crate) fn init_dynamic(base: usize, len: usize) {
    ALLOCATOR.init_dynamic(base, len);
}

/// 注册按需增长/归还回调；必须在 System Capacity 与 direct map 建立后调用。
pub(crate) fn register_grower() {
    ALLOCATOR.set_grower(grow, shrink);
}

/// 从 FrameBank 取一段连续物理页，按 direct map 换成虚拟地址交给内核堆。
fn grow(len: usize) -> Option<(usize, usize)> {
    let direct = hk_arch_x86_64::paging::direct_map_base();
    if direct == 0 {
        return None;
    }
    let range =
        hk_arch_x86_64::paging::FRAME_BANK.allocate_contiguous_for(FrameKind::KernelHeap, len)?;
    hk_arch_x86_64::system_arena::register_kernel_range(range.start(), range.len(), direct);
    Some((
        direct.saturating_add(range.start().get()) as usize,
        range.len() as usize,
    ))
}

/// 把完全空闲的 chunk 归还 FrameBank。
fn shrink(base: usize, len: usize) {
    let direct = hk_arch_x86_64::paging::direct_map_base();
    let Some(phys) = (base as u64).checked_sub(direct) else {
        return;
    };
    if len == 0 || !phys.is_multiple_of(PAGE_SIZE as u64) {
        return;
    }
    hk_arch_x86_64::system_arena::unregister_kernel_range(PhysAddr::new(phys), len as u64, direct);
    hk_arch_x86_64::paging::FRAME_BANK.free_for(
        FrameKind::KernelHeap,
        PhysRange::new(PhysAddr::new(phys), len as u64),
    );
}
