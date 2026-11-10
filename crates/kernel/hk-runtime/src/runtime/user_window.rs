//! 每个任务的自动用户虚拟地址窗口。

use alloc::vec::Vec;

use hk_base::user::{AUTO_WINDOW_BASE, AUTO_WINDOW_LIMIT};
use hk_hal::PAGE_SIZE;

/// 一段已分配的虚拟区间。
type Range = (u64, u64);

/// 任务私有的自动虚拟地址窗口。
pub(crate) struct UserWindow {
    next: u64,
    limit: u64,
    free: Vec<Range>,
    allocated: Vec<Range>,
}

impl UserWindow {
    /// 以默认用户窗口构造。
    pub(crate) const fn new() -> Self {
        Self {
            next: AUTO_WINDOW_BASE,
            limit: AUTO_WINDOW_LIMIT,
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

    /// 扩展一段已分配的虚拟区间。
    ///
    /// 扩长只在尾部追加：可以消耗紧邻的空闲区间，或推进 bump 指针；
    /// 不能跨越其他已分配区间。
    pub(crate) fn grow(&mut self, virt: u64, old_len: usize, new_len: usize) -> bool {
        let Some(old) = align(old_len) else {
            return false;
        };
        let Some(new) = align(new_len) else {
            return false;
        };
        if new <= old {
            return false;
        }
        let Some(index) = self
            .allocated
            .iter()
            .position(|(start, size)| *start == virt && *size == old)
        else {
            return false;
        };
        let Some(start) = virt.checked_add(old) else {
            return false;
        };
        let Some(end) = virt.checked_add(new) else {
            return false;
        };
        if end > self.limit {
            return false;
        }
        if self
            .allocated
            .iter()
            .enumerate()
            .any(|(other, (s, size))| other != index && *s < end && start < s.saturating_add(*size))
        {
            return false;
        }
        if start < self.next {
            let Some(free_index) = self
                .free
                .iter()
                .position(|(s, size)| *s <= start && s.saturating_add(*size) >= end)
            else {
                return false;
            };
            let (free_start, free_size) = self.free.swap_remove(free_index);
            let free_end = free_start.saturating_add(free_size);
            if free_start < start {
                self.free.push((free_start, start - free_start));
            }
            if end < free_end {
                self.free.push((end, free_end - end));
            }
        } else if start != self.next {
            return false;
        } else {
            self.next = end;
        }
        self.allocated[index].1 = new;
        true
    }

    /// 回退一次 `grow`，供映射失败时恢复窗口状态。
    pub(crate) fn shrink(&mut self, virt: u64, old_len: usize, new_len: usize) -> bool {
        let Some(old) = align(old_len) else {
            return false;
        };
        let Some(new) = align(new_len) else {
            return false;
        };
        if new >= old {
            return false;
        }
        let Some(index) = self
            .allocated
            .iter()
            .position(|(start, size)| *start == virt && *size == old)
        else {
            return false;
        };
        self.allocated[index].1 = new;
        let Some(old_end) = virt.checked_add(old) else {
            return false;
        };
        if self.next == old_end {
            self.next = virt.saturating_add(new);
        } else {
            self.free.push((virt.saturating_add(new), old - new));
            self.coalesce();
        }
        true
    }

    /// 登记一段外部已建立的映射，避免自动窗口再次分配到这里。
    ///
    /// fork 子任务继承父任务已映射的 Region，但子任务的自动窗口仍然从
    /// `AUTO_WINDOW_BASE` 开始；不登记就会在后续 `SYS_REGION_MAP` 时覆盖
    /// 继承下来的 PTE。
    pub(crate) fn reserve(&mut self, virt: u64, length: usize) -> bool {
        let Some(length) = align(length) else {
            return false;
        };
        let Some(end) = virt.checked_add(length) else {
            return false;
        };
        if virt < AUTO_WINDOW_BASE || end > self.limit {
            return false;
        }
        if self
            .allocated
            .iter()
            .any(|(start, size)| virt < start.saturating_add(*size) && *start < end)
        {
            return false;
        }
        // 保守处理：与自由区间重叠时直接丢弃该自由区间，避免拆分逻辑出错。
        self.free.retain(|(start, size)| {
            let free_end = start.saturating_add(*size);
            !(virt < free_end && *start < end)
        });
        // 保守推进 bump 指针：宁可跳过区间下方的空洞，也不让后续分配
        // 覆盖已继承的映射。
        self.next = self.next.max(end);
        self.allocated.push((virt, length));
        true
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
    use super::UserWindow;

    #[test]
    fn allocates_page_aligned_ranges() {
        let mut window = UserWindow::with_window(0x1000_0000, 0x1001_0000);
        let first = window.allocate(100).unwrap();
        let second = window.allocate(5000).unwrap();
        assert_eq!(first, 0x1000_0000);
        assert_eq!(second, 0x1000_1000);
    }

    #[test]
    fn reuses_released_ranges_and_coalesces() {
        let mut window = UserWindow::with_window(0x1000_0000, 0x1001_0000);
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
    fn reserves_inherited_ranges() {
        let base = hk_base::user::AUTO_WINDOW_BASE;
        let mut window = UserWindow::with_window(base, base + 0x1_0000);
        assert!(window.reserve(base, 0x2000));
        assert_eq!(window.allocate(0x1000), Some(base + 0x2000));
        assert!(!window.reserve(base + 0x1000, 0x1000));
    }

    #[test]
    fn ignores_unknown_releases() {
        let mut window = UserWindow::with_window(0x1000_0000, 0x1001_0000);
        window.release(0x2000_0000);
        assert_eq!(window.allocate(0x1000), Some(0x1000_0000));
    }

    #[test]
    fn returns_none_when_window_is_exhausted() {
        let mut window = UserWindow::with_window(0x1000_0000, 0x1000_2000);
        assert_eq!(window.allocate(0x1000), Some(0x1000_0000));
        assert_eq!(window.allocate(0x1000), Some(0x1000_1000));
        assert_eq!(window.allocate(0x1000), None);
    }
}
