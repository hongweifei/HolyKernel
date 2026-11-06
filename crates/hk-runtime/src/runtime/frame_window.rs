//! 每个任务的自动 Frame 虚拟地址窗口。

use alloc::vec::Vec;

use hk_hal::PAGE_SIZE;
use hk_syscall::{USER_FRAME_WINDOW_BASE, USER_FRAME_WINDOW_LIMIT};

/// 一段已分配的虚拟区间。
type Range = (u64, u64);

/// 任务私有的 Frame 虚拟地址窗口。
pub(crate) struct FrameWindow {
    next: u64,
    limit: u64,
    free: Vec<Range>,
    allocated: Vec<Range>,
}

impl FrameWindow {
    /// 以默认用户窗口构造。
    pub(crate) const fn new() -> Self {
        Self {
            next: USER_FRAME_WINDOW_BASE,
            limit: USER_FRAME_WINDOW_LIMIT,
            free: Vec::new(),
            allocated: Vec::new(),
        }
    }

    /// 以指定窗口构造，供测试使用。
    #[cfg(test)]
    fn with_window(base: u64, limit: u64) -> Self {
        Self {
            next: base,
            limit,
            free: Vec::new(),
            allocated: Vec::new(),
        }
    }

    /// 分配一段页对齐的虚拟区间；窗口耗尽时返回 `None`。
    pub(crate) fn allocate(&mut self, length: usize) -> Option<u64> {
        let length = align(length)?;
        let start = if let Some(index) = self.free.iter().position(|(_, size)| *size >= length) {
            let (start, size) = self.free.swap_remove(index);
            if size > length {
                self.free.push((start + length, size - length));
            }
            start
        } else {
            let end = self.next.checked_add(length)?;
            if end > self.limit {
                return None;
            }
            let start = self.next;
            self.next = end;
            start
        };
        self.allocated.push((start, length));
        Some(start)
    }

    /// 归还一段自动分配的虚拟区间；未登记过的地址会被忽略。
    pub(crate) fn release(&mut self, virt: u64) {
        let Some(index) = self.allocated.iter().position(|(start, _)| *start == virt) else {
            return;
        };
        let range = self.allocated.swap_remove(index);
        self.free.push(range);
        self.coalesce();
    }

    fn coalesce(&mut self) {
        self.free.sort_unstable_by_key(|(start, _)| *start);
        let mut merged: Vec<Range> = Vec::with_capacity(self.free.len());
        for (start, size) in self.free.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.0.saturating_add(last.1) == start
            {
                last.1 = last.1.saturating_add(size);
                continue;
            }
            merged.push((start, size));
        }
        self.free = merged;
    }
}

/// 把长度向上对齐到页边界。
fn align(length: usize) -> Option<u64> {
    let page = PAGE_SIZE as u64;
    let length = u64::try_from(length).ok()?;
    let aligned = length.checked_add(page.checked_sub(1)?)? / page * page;
    (aligned != 0).then_some(aligned)
}

#[cfg(test)]
mod tests {
    use super::FrameWindow;

    #[test]
    fn allocates_page_aligned_ranges() {
        let mut window = FrameWindow::with_window(0x1000_0000, 0x1001_0000);
        let first = window.allocate(100).unwrap();
        let second = window.allocate(5000).unwrap();
        assert_eq!(first, 0x1000_0000);
        assert_eq!(second, 0x1000_1000);
    }

    #[test]
    fn reuses_released_ranges_and_coalesces() {
        let mut window = FrameWindow::with_window(0x1000_0000, 0x1001_0000);
        let first = window.allocate(0x1000).unwrap();
        let second = window.allocate(0x1000).unwrap();
        let third = window.allocate(0x1000).unwrap();
        window.release(second);
        assert_eq!(window.allocate(0x1000), Some(second));
        window.release(first);
        window.release(second);
        assert_eq!(window.allocate(0x2000), Some(first));
        assert_eq!(third, 0x1000_2000);
    }

    #[test]
    fn ignores_unknown_releases() {
        let mut window = FrameWindow::with_window(0x1000_0000, 0x1001_0000);
        window.release(0x2000_0000);
        assert_eq!(window.allocate(0x1000), Some(0x1000_0000));
    }

    #[test]
    fn returns_none_when_window_is_exhausted() {
        let mut window = FrameWindow::with_window(0x1000_0000, 0x1000_2000);
        assert_eq!(window.allocate(0x1000), Some(0x1000_0000));
        assert_eq!(window.allocate(0x1000), Some(0x1000_1000));
        assert_eq!(window.allocate(0x1000), None);
    }
}
