//! 启动期预留的连续 DMA 池。

use hk_hal::{PAGE_SIZE, PhysRange};
use spin::Mutex;

struct Pool {
    next: u64,
    end: u64,
}

static POOL: Mutex<Option<Pool>> = Mutex::new(None);

/// 注册启动期预留的连续 DMA 区域。
pub fn init(range: PhysRange) {
    *POOL.lock() = Some(Pool {
        next: range.start().get(),
        end: range.end().get(),
    });
}

/// 从 DMA 池分配一段连续内存。
pub fn allocate(len: usize) -> Option<PhysRange> {
    let page = PAGE_SIZE as u64;
    let length = (len as u64).div_ceil(page) * page;
    if length == 0 {
        return None;
    }
    let mut pool = POOL.lock();
    let pool = pool.as_mut()?;
    if pool.next.checked_add(length)? > pool.end {
        return None;
    }
    let start = pool.next;
    pool.next += length;
    Some(PhysRange::new(hk_hal::PhysAddr::new(start), length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::PhysAddr;

    #[test]
    fn allocates_contiguous_pages() {
        init(PhysRange::new(PhysAddr::new(0x10_0000), 0x4000));
        let first = allocate(4096).expect("第一页");
        let second = allocate(8192).expect("第二段");
        assert_eq!(first.start(), PhysAddr::new(0x10_0000));
        assert_eq!(first.len(), 4096);
        assert_eq!(second.start(), PhysAddr::new(0x10_1000));
        assert_eq!(second.len(), 8192);
    }
}
