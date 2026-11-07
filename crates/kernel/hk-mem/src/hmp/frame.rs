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

    /// 固定 Frame，供 DMA/scanout 使用。
    pub fn pin(&mut self) {
        self.pinned = true;
    }

    /// 解除固定。
    pub fn unpin(&mut self) {
        self.pinned = false;
    }
}
