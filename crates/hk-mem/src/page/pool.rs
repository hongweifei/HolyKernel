//! 预留物理帧池：写时复制与页表等固定用途。

use alloc::vec::Vec;

use hk_hal::PhysAddr;
use spin::Mutex;

use super::linear::LinearPages;

/// 固定用途的物理帧池。
pub struct PagePool {
    frames: Mutex<Vec<PhysAddr>>,
}

impl PagePool {
    /// 构造空池。
    pub const fn new() -> Self {
        Self {
            frames: Mutex::new(Vec::new()),
        }
    }

    /// 从线性分配器预留 `count` 个帧。
    pub fn reserve(&self, allocator: &mut LinearPages, count: usize) {
        let mut frames = self.frames.lock();
        for _ in 0..count {
            let Some(frame) = allocator.allocate() else {
                break;
            };
            frames.push(frame);
        }
    }

    /// 取出一个帧。
    pub fn take(&self) -> Option<PhysAddr> {
        self.frames.lock().pop()
    }

    /// 归还一个帧。
    pub fn give(&self, frame: PhysAddr) {
        self.frames.lock().push(frame);
    }

    /// 返回当前可用帧数。
    pub fn len(&self) -> usize {
        self.frames.lock().len()
    }

    /// 判断池是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for PagePool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_and_returns_frames() {
        let pool = PagePool::new();
        let mut frames = LinearPages::from_range(0x10_0000, 0x10_2000);
        pool.reserve(&mut frames, 2);
        assert_eq!(pool.len(), 2);
        let first = pool.take().expect("第一个帧");
        let second = pool.take().expect("第二个帧");
        assert_ne!(first, second);
        pool.give(first);
        assert_eq!(pool.take(), Some(first));
        assert!(pool.is_empty());
    }
}
