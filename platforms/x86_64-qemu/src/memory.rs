//! 启动期物理内存接管：帧银行、内核堆、DMA 窗口与 System Capacity。
//!
//! 顺序固定为：帧银行 → 内核堆 → 物理后端 → DMA 预留 → System Capacity。
//! 之后内核与用户态分配都按容量归因，启动日志由 [`Memory`] 描述。

use alloc::string::String;
use core::fmt;

use hk_mem::hmp::CapacityId;

/// 启动期内核堆字节数。
const KERNEL_HEAP_BYTES: usize = 16 * 1024 * 1024;

/// 启动完成后的内存快照。
pub(crate) struct Memory {
    stats: hk_mem::FrameBankStats,
    system_used: u64,
    system_limit: u64,
    system_span: u64,
}

impl fmt::Display for Memory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "System Capacity：used {} KiB / total {} KiB，peak {} KiB；system {} / {} KiB（窗口 {} KiB）；heap {}，page-table {}，cow {}，restore {}，image {}，dma {}，other {}",
            self.stats.used / 1024,
            self.stats.total / 1024,
            self.stats.peak / 1024,
            self.system_used / 1024,
            self.system_limit / 1024,
            self.system_span / 1024,
            self.stats.by_kind[0] / 1024,
            self.stats.by_kind[1] / 1024,
            self.stats.by_kind[2] / 1024,
            self.stats.by_kind[3] / 1024,
            self.stats.by_kind[4] / 1024,
            self.stats.by_kind[5] / 1024,
            self.stats.by_kind[6] / 1024,
        )
    }
}

/// 接管全部可用物理内存并建立 System Capacity。
pub(crate) fn bring_up(info: &hk_hal::BootInfo, direct_map_base: u64) -> Result<Memory, String> {
    let bank = &hk_platform_x86_64::paging::FRAME_BANK;
    bank.init(info.memory_map, info.kernel_image);
    if bank.total() == 0 {
        return Err(String::from("没有可用的物理内存区域"));
    }
    let heap = bank
        .allocate_contiguous_for(hk_mem::FrameKind::KernelHeap, KERNEL_HEAP_BYTES)
        .ok_or_else(|| String::from("内核堆物理内存不足"))?;
    let heap_virt = direct_map_base.saturating_add(heap.start().get());
    // SAFETY: heap 来自 FrameBank，direct map 已覆盖该物理区间。
    unsafe {
        core::ptr::write_bytes(heap_virt as *mut u8, 0, KERNEL_HEAP_BYTES);
    }
    crate::allocator::init_dynamic(heap_virt as usize, KERNEL_HEAP_BYTES);

    hk_platform_x86_64::paging::init_runtime(direct_map_base);
    hk_platform_x86_64::paging::register_backend();
    hk_platform_x86_64::cache::init(direct_map_base);

    let dma_capacity = CapacityId::next();
    let mut dma_bytes = 0;
    if let Some(dma) = bank.reserve_for(dma_capacity, hk_platform_x86_64::dma::POOL_BYTES) {
        dma_bytes = dma.len();
        hk_platform_x86_64::dma::init(dma_capacity, dma);
    }
    hk_platform_x86_64::system::establish(bank, heap, dma_bytes)
        .ok_or_else(|| String::from("无法从可用内存建立 System Capacity"))?;

    let stats = bank.stats();
    let (system_used, system_limit, system_span) =
        hk_platform_x86_64::system::capacity().map_or((0, 0, 0), |capacity| {
            let capacity = capacity.lock();
            (capacity.used(), capacity.limit(), capacity.range().len())
        });
    Ok(Memory {
        stats,
        system_used,
        system_limit,
        system_span,
    })
}
