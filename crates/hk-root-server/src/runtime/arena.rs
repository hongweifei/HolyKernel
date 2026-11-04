//! 任务 Arena：线性分配，不回收。

use super::*;

impl Arena {
    /// 以基址与大小构造 Arena。
    pub const fn new(base: u64, size: u64) -> Self {
        Self {
            base,
            next: base,
            end: base + size,
        }
    }

    /// 分配一段对齐区间，返回用户虚拟地址。
    pub fn allocate(&mut self, size: usize, align: usize) -> Option<u64> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }

        let align = align.max(8) as u64;
        let start = (self.next + align - 1) & !(align - 1);
        let end = start.checked_add(size as u64)?;
        if end > self.end {
            return None;
        }
        self.next = end;
        Some(start)
    }

    /// 返回已用字节数。
    pub fn used(&self) -> u64 {
        self.next - self.base
    }

    /// 返回 Arena 基址。
    pub fn base(&self) -> u64 {
        self.base
    }

    /// 返回总字节数。
    pub fn size(&self) -> u64 {
        self.end - self.base
    }
}
