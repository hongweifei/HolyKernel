//! 帧引用计数：写时复制的唯一共享状态。

use alloc::collections::BTreeMap;

use hk_hal::PhysAddr;

/// 同时跟踪的共享帧上限，避免固件/异常路径无界增长。
pub const MAX_TRACKED_FRAMES: usize = 1 << 20;

/// 物理帧引用表。
#[derive(Clone, Debug, Default)]
pub struct FrameRefs {
    counts: BTreeMap<u64, u32>,
}

impl FrameRefs {
    /// 构造空引用表。
    pub const fn new() -> Self {
        Self {
            counts: BTreeMap::new(),
        }
    }

    /// 增加一帧的共享引用；达到上限时返回 `false`。
    pub fn acquire(&mut self, frame: PhysAddr) -> bool {
        self.acquire_raw(frame.get())
    }

    /// 增加一个物理地址的共享引用；达到上限时返回 `false`。
    pub fn acquire_raw(&mut self, frame: u64) -> bool {
        if !self.counts.contains_key(&frame) && self.counts.len() >= MAX_TRACKED_FRAMES {
            return false;
        }
        match self.counts.entry(frame) {
            alloc::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(1);
            }
            alloc::collections::btree_map::Entry::Occupied(mut entry) => {
                *entry.get_mut() = entry.get().saturating_add(1);
            }
        }
        true
    }

    /// 释放一帧的共享引用，返回剩余引用数。
    ///
    /// 返回 0 表示该帧已不再共享，调用方可以归还物理帧。
    pub fn release(&mut self, frame: PhysAddr) -> u32 {
        self.release_raw(frame.get())
    }

    /// 释放一个物理地址的共享引用，返回剩余引用数。
    pub fn release_raw(&mut self, frame: u64) -> u32 {
        let Some(count) = self.counts.get_mut(&frame) else {
            return 0;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.counts.remove(&frame);
            0
        } else {
            *count
        }
    }

    /// 返回一帧当前的共享引用数；未登记时为 0。
    pub fn count(&self, frame: PhysAddr) -> u32 {
        self.counts.get(&frame.get()).copied().unwrap_or(0)
    }

    /// 返回一个物理地址当前的共享引用数；未登记时为 0。
    pub fn count_raw(&self, frame: u64) -> u32 {
        self.counts.get(&frame).copied().unwrap_or(0)
    }

    /// 判断一帧是否仍被至少两个地址空间共享。
    pub fn is_shared(&self, frame: PhysAddr) -> bool {
        self.count(frame) > 1
    }

    /// 清空引用表。
    pub fn clear(&mut self) {
        self.counts.clear();
    }

    /// 当前跟踪的帧数。
    pub fn tracked(&self) -> usize {
        self.counts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_and_releases_shared_frames() {
        let mut refs = FrameRefs::new();
        let frame = PhysAddr::new(0x1000);
        assert_eq!(refs.count(frame), 0);
        assert!(refs.acquire(frame));
        assert_eq!(refs.count(frame), 1);
        assert!(refs.acquire(frame));
        assert_eq!(refs.count(frame), 2);
        assert!(refs.is_shared(frame));

        assert_eq!(refs.release(frame), 1);
        assert!(!refs.is_shared(frame));
        assert_eq!(refs.release(frame), 0);
        assert_eq!(refs.count(frame), 0);
        assert_eq!(refs.tracked(), 0);
    }

    #[test]
    fn release_untracked_frame_is_idempotent() {
        let mut refs = FrameRefs::new();
        assert_eq!(refs.release(PhysAddr::new(0x2000)), 0);
        assert_eq!(refs.release(PhysAddr::new(0x2000)), 0);
    }
}
