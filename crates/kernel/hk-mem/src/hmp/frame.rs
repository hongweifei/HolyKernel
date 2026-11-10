//! 物理页能力。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::PhysAddr;

use super::{CapacityId, HmpError};

static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);

/// Frame 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameId(u64);

impl FrameId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 连续物理页。
pub struct Frame {
    id: FrameId,
    phys: PhysAddr,
    len: usize,
    page_size: usize,
    capacity: CapacityId,
    pinned: bool,
    refs: u32,
}

impl Frame {
    pub(crate) fn new(
        phys: PhysAddr,
        len: usize,
        page_size: usize,
        capacity: CapacityId,
    ) -> Result<Self, HmpError> {
        if len == 0 || page_size == 0 || !len.is_multiple_of(page_size) {
            return Err(HmpError::InvalidSize);
        }
        Ok(Self {
            id: FrameId::new(NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed)),
            phys,
            len,
            page_size,
            capacity,
            pinned: false,
            refs: 1,
        })
    }

    /// 返回 Frame 标识。
    pub const fn id(&self) -> FrameId {
        self.id
    }

    /// 返回起始物理地址。
    pub const fn phys(&self) -> PhysAddr {
        self.phys
    }

    /// 返回字节长度。
    pub const fn len(&self) -> usize {
        self.len
    }

    /// 判断长度是否为零。
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回页大小。
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// 返回所属 Capacity。
    pub const fn capacity(&self) -> CapacityId {
        self.capacity
    }

    /// 判断是否保持驻留。
    pub const fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// 返回共享引用数。
    pub const fn refs(&self) -> u32 {
        self.refs
    }

    /// 增加共享引用。
    pub fn share(&mut self) -> Result<u32, HmpError> {
        self.refs = self.refs.checked_add(1).ok_or(HmpError::OutOfRange)?;
        Ok(self.refs)
    }

    /// 释放一次共享引用，返回剩余引用数。
    pub fn release(&mut self) -> u32 {
        self.refs = self.refs.saturating_sub(1);
        self.refs
    }

    /// 固定 Frame，供 DMA/scanout 使用。
    pub fn pin(&mut self) {
        self.pinned = true;
    }

    /// 解除固定。
    pub fn unpin(&mut self) {
        self.pinned = false;
    }

    /// 对象最后一次销毁时把残余共享引用视为唯一引用。
    ///
    /// 只有该 Frame 的最后一个 Handle 被销毁时才会调用；此时仍大于 1 的计数
    /// 是未归还的显式 share，不能阻止物理页归还。
    pub fn clear_refs(&mut self) {
        self.refs = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame::new(PhysAddr::new(0x20_0000), 0x1000, 0x1000, CapacityId::new(1)).expect("Frame")
    }

    #[test]
    fn shares_and_releases_references() {
        let mut frame = frame();
        assert_eq!(frame.refs(), 1);
        assert_eq!(frame.share(), Ok(2));
        assert_eq!(frame.release(), 1);
        assert_eq!(frame.release(), 0);
    }

    #[test]
    fn validates_frame_geometry() {
        assert_eq!(
            Frame::new(PhysAddr::new(0x20_0000), 0, 0x1000, CapacityId::new(1)).err(),
            Some(HmpError::InvalidSize)
        );
        assert_eq!(
            Frame::new(PhysAddr::new(0x20_0000), 0x1800, 0x1000, CapacityId::new(1)).err(),
            Some(HmpError::InvalidSize)
        );
    }
}
