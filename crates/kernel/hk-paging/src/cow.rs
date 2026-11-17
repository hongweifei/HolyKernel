//! 帧引用计数：映射与 COW 共用全局 FrameLedger。
//!
//! 本类型是无状态 facade；真正的计数在 `hk-mem` 的全局 `FrameLedger`，避免
//! 平台映射与任务记账各维护一套引用。

use hk_hal::PhysAddr;

use hk_mem::hmp::frame_ledger;

/// 物理帧引用 facade。
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameRefs;

impl FrameRefs {
    /// 构造 facade。
    pub const fn new() -> Self {
        Self
    }

    /// 增加一帧的共享引用。
    pub fn acquire(&mut self, frame: PhysAddr) -> bool {
        self.acquire_raw(frame.get())
    }

    /// 增加一个物理地址的共享引用。
    pub fn acquire_raw(&mut self, frame: u64) -> bool {
        frame_ledger::mapping_acquire(frame)
    }

    /// 释放一帧的共享引用，返回剩余引用数。
    pub fn release(&mut self, frame: PhysAddr) -> u32 {
        self.release_raw(frame.get())
    }

    /// 释放一个物理地址的共享引用，返回剩余引用数。
    pub fn release_raw(&mut self, frame: u64) -> u32 {
        frame_ledger::mapping_release(frame)
    }

    /// 返回一帧当前的共享引用数。
    pub fn count(&self, frame: PhysAddr) -> u32 {
        self.count_raw(frame.get())
    }

    /// 返回一个物理地址当前的共享引用数。
    pub fn count_raw(&self, frame: u64) -> u32 {
        frame_ledger::mapping_count(frame)
    }

    /// 判断一帧是否仍被至少两个地址空间共享。
    pub fn is_shared(&self, frame: PhysAddr) -> bool {
        self.count(frame) > 1
    }

    /// 清空全局引用表；仅测试使用。
    pub fn clear(&mut self) {
        frame_ledger::mapping_clear();
    }

    /// 当前跟踪的帧数。
    pub fn tracked(&self) -> usize {
        frame_ledger::mapping_tracked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_and_releases_shared_frames() {
        let mut refs = FrameRefs::new();
        let frame = PhysAddr::new(0x1000);
        frame_ledger::mapping_clear();
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
        frame_ledger::mapping_clear();
        assert_eq!(refs.release(PhysAddr::new(0x2000)), 0);
        assert_eq!(refs.release(PhysAddr::new(0x2000)), 0);
    }
}
