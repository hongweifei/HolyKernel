//! x86_64 缓存维护。
//!
//! x86 对普通内存通常保持设备一致性；这里仍提供显式 `clflush`，用于
//! 非一致 DMA 或设备可见缓冲。

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{CacheOps, PhysRange};

static DIRECT_MAP: AtomicU64 = AtomicU64::new(0);

/// 设置 direct map 基址。
pub fn init(direct_map: u64) {
    DIRECT_MAP.store(direct_map, Ordering::Release);
}

/// x86_64 缓存维护实现。
pub struct X86Cache;

/// 全局缓存维护实例。
pub static CACHE: X86Cache = X86Cache;

impl CacheOps for X86Cache {
    fn clean(&self, range: PhysRange) {
        flush_range(range, true);
    }

    fn invalidate(&self, range: PhysRange) {
        flush_range(range, false);
    }

    fn barrier(&self) {
        hk_arch::fence();
    }
}

fn flush_range(range: PhysRange, writeback: bool) {
    let direct_map = DIRECT_MAP.load(Ordering::Acquire);
    if direct_map == 0 || range.is_empty() {
        return;
    }
    let mut address = range.start().get() & !0x3F;
    let end = range.end().get();
    while address < end {
        let pointer = direct_map + address;
        // SAFETY: direct map 覆盖物理内存；clflush 只作用于缓存行。
        unsafe {
            let _ = writeback;
            asm!("clflush [{}]", in(reg) pointer, options(nostack, preserves_flags));
        }
        address += 64;
    }
    hk_arch::fence();
}
