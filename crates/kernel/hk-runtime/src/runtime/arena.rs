//! 任务 Arena：线性分配，不回收。

/// 任务 Arena：线性分配，不回收。
#[derive(Clone, Copy)]
pub struct Arena {
    base: u64,
    next: u64,
    end: u64,
}

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
    ///
    /// 区间按页对齐并按整页推进：Arena 由惰性匿名 Region 覆盖，按页划分后
    /// 每次分配都能独立缺页装入，不会与相邻分配共享同一页。
    pub fn allocate(&mut self, size: usize, align: usize) -> Option<u64> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }

        let page = hk_hal::PAGE_SIZE as u64;
        let align = (align.max(8) as u64).max(page);
        let size = (size as u64).div_ceil(page) * page;
        let start = (self.next + align - 1) & !(align - 1);
        let end = start.checked_add(size)?;
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

    /// 恢复已用字节数；用于检查点恢复。
    pub(crate) fn set_used(&mut self, used: u64) {
        self.next = self.base.saturating_add(used).min(self.end);
    }

    /// 返回总字节数。
    pub fn size(&self) -> u64 {
        self.end - self.base
    }
}
