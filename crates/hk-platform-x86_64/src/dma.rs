//! 启动期预留的连续 DMA 池。

use alloc::vec::Vec;

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

struct Pool {
    next: u64,
    end: u64,
}

static POOL: Mutex<Option<Pool>> = Mutex::new(None);
static FREE: Mutex<Vec<PhysRange>> = Mutex::new(Vec::new());

/// 注册启动期预留的连续 DMA 区域。
pub fn init(range: PhysRange) {
    FREE.lock().clear();
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
    let mut free = FREE.lock();
    if let Some(index) = free.iter().position(|range| range.len() >= length) {
        let range = free.swap_remove(index);
        let start = range.start();
        if range.len() > length {
            free.push(PhysRange::new(
                PhysAddr::new(start.get() + length),
                range.len() - length,
            ));
        }
        return Some(PhysRange::new(start, length));
    }

    let mut pool = POOL.lock();
    let pool = pool.as_mut()?;
    if pool.next.checked_add(length)? > pool.end {
        return None;
    }
    let start = pool.next;
    pool.next += length;
    Some(PhysRange::new(PhysAddr::new(start), length))
}

/// 归还一段 DMA 内存。
pub fn free(range: PhysRange) {
    if range.is_empty() {
        return;
    }
    FREE.lock().push(range);
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

        free(first);
        assert_eq!(
            allocate(4096).expect("复用").start(),
            PhysAddr::new(0x10_0000)
        );
    }
}
