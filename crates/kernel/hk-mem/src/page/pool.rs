//! 内核页分配器：按用途从物理帧银行取页与归还。
//!
//! 页表、CoW 与检查点恢复各自使用固定的 `FrameKind`；把这类选择留在
//! `hk-mem`，平台侧只负责页表机制，不再自行决定从哪类池取页。

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};

use super::{FrameBank, FrameKind};

/// 按用途分类的内核页分配器。
pub struct PageAllocator<'a> {
    bank: &'a FrameBank,
}

impl<'a> PageAllocator<'a> {
    /// 包装一个物理帧银行。
    pub const fn new(bank: &'a FrameBank) -> Self {
        Self { bank }
    }

    /// 取一页页表。
    pub fn page_table(&self) -> Option<PhysAddr> {
        self.bank.allocate_for(FrameKind::PageTable)
    }

    /// 取一页写时复制帧。
    pub fn cow(&self) -> Option<PhysAddr> {
        self.bank.allocate_for(FrameKind::Cow)
    }

    /// 取一页检查点恢复帧。
    pub fn restore(&self) -> Option<PhysAddr> {
        self.bank.allocate_for(FrameKind::Restore)
    }

    /// 归还一页检查点恢复帧。
    pub fn free_restore(&self, frame: PhysAddr) {
        self.bank
            .free_for(FrameKind::Restore, PhysRange::new(frame, PAGE_SIZE as u64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_pages_from_their_kinds() {
        let bank = FrameBank::from_range(PAGE_SIZE as u64, 4 * PAGE_SIZE as u64);
        let pool = PageAllocator::new(&bank);

        let table = pool.page_table().expect("页表页");
        let cow = pool.cow().expect("CoW 页");
        assert_ne!(table, cow);

        let restore = pool.restore().expect("恢复页");
        pool.free_restore(restore);
        assert!(pool.restore().is_some());
    }
}
