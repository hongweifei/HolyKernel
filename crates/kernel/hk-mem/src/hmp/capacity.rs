//! 物理容量能力。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{PhysAddr, PhysRange};

use super::{Frame, HmpError};

static NEXT_CAPACITY_ID: AtomicU64 = AtomicU64::new(1);

/// Capacity 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapacityId(u64);

impl CapacityId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 分配一个进程内唯一标识。
    pub fn next() -> Self {
        Self(NEXT_CAPACITY_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Capacity 用途。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityKind {
    /// 内核系统容量。
    System,
    /// 用户任务容量。
    User,
    /// DMA 容量。
    Dma,
    /// GPU/显存容量。
    Gpu,
    /// NUMA 节点容量。
    Numa(u8),
}

/// Root Server 授予的物理容量能力。
pub struct Capacity {
    id: CapacityId,
    range: PhysRange,
    page_size: usize,
    kind: CapacityKind,
    limit: u64,
    used: u64,
    free: Vec<PhysRange>,
    next: u64,
}

impl Capacity {
    /// 以物理区间、页大小和配额创建 Capacity。
    pub fn new(
        id: CapacityId,
        range: PhysRange,
        page_size: usize,
        kind: CapacityKind,
        limit: u64,
    ) -> Result<Self, HmpError> {
        if page_size == 0 || !page_size.is_power_of_two() || range.is_empty() {
            return Err(HmpError::InvalidSize);
        }
        if !range.start().get().is_multiple_of(page_size as u64) {
            return Err(HmpError::Unaligned);
        }
        let available = range.len();
        let limit = if limit == 0 {
            available
        } else {
            limit.min(available)
        };
        if limit < page_size as u64 {
            return Err(HmpError::InvalidSize);
        }
        Ok(Self {
            id,
            range,
            page_size,
            kind,
            limit,
            used: 0,
            free: Vec::new(),
            next: range.start().get(),
        })
    }

    /// 分配一段连续物理页，返回 Frame。
    pub fn allocate(&mut self, len: usize) -> Result<Frame, HmpError> {
        let length = align_up(len, self.page_size).ok_or(HmpError::InvalidSize)?;
        if length == 0 || self.used.saturating_add(length) > self.limit {
            return Err(HmpError::OutOfCapacity);
        }

        let start = if let Some(index) = self.free.iter().position(|range| range.len() >= length) {
            let range = self.free.swap_remove(index);
            let start = range.start().get();
            let remaining = range.len() - length;
            if remaining > 0 {
                self.free
                    .push(PhysRange::new(PhysAddr::new(start + length), remaining));
            }
            start
        } else {
            let end = self
                .next
                .checked_add(length)
                .ok_or(HmpError::OutOfCapacity)?;
            let limit_end = self.range.start().get().saturating_add(self.limit);
            if end > limit_end {
                return Err(HmpError::OutOfCapacity);
            }
            let start = self.next;
            self.next = end;
            start
        };

        let frame = Frame::new(
            PhysAddr::new(start),
            length as usize,
            self.page_size,
            self.id,
        )?;
        self.used = self.used.saturating_add(length);
        Ok(frame)
    }

    /// 从未分配尾部切出一个子 Capacity。
    ///
    /// 分割把尾部区间从当前 Capacity 的可用空间中移除；子 Capacity 从切出
    /// 区间的起点开始分配。
    pub fn split(
        &mut self,
        id: CapacityId,
        len: usize,
        kind: CapacityKind,
    ) -> Result<Self, HmpError> {
        let length = align_up(len, self.page_size).ok_or(HmpError::InvalidSize)?;
        if length == 0 {
            return Err(HmpError::InvalidSize);
        }
        let start = self.next;
        let end = start.checked_add(length).ok_or(HmpError::OutOfCapacity)?;
        let limit_end = self.range.start().get().saturating_add(self.limit);
        if end > limit_end {
            return Err(HmpError::OutOfCapacity);
        }
        self.next = end;
        self.used = self.used.saturating_add(length);
        Self::new(
            id,
            PhysRange::new(PhysAddr::new(start), length),
            self.page_size,
            kind,
            length,
        )
    }

    /// 归还 Frame 的物理区间。
    pub fn free(&mut self, frame: Frame) -> Result<(), HmpError> {
        if frame.capacity() != self.id {
            return Err(HmpError::WrongCapability);
        }
        if frame.is_pinned() {
            return Err(HmpError::PermissionDenied);
        }
        if frame.refs() != 1 {
            return Err(HmpError::PermissionDenied);
        }
        self.release(frame.phys(), frame.len())
    }

    /// 归还一段原始物理区间，供底层 DMA 池使用。
    pub fn release(&mut self, phys: PhysAddr, len: usize) -> Result<(), HmpError> {
        let length = align_up(len, self.page_size).ok_or(HmpError::InvalidSize)?;
        if length == 0 || !phys.get().is_multiple_of(self.page_size as u64) {
            return Err(HmpError::Unaligned);
        }
        let start = phys.get();
        let end = start.checked_add(length).ok_or(HmpError::OutOfRange)?;
        let limit_end = self.range.start().get().saturating_add(self.limit);
        if start < self.range.start().get() || end > limit_end {
            return Err(HmpError::OutOfRange);
        }
        self.used = self.used.saturating_sub(length);
        self.free.push(PhysRange::new(phys, length));
        self.coalesce();
        Ok(())
    }

    /// 返回 Capacity 标识。
    pub const fn id(&self) -> CapacityId {
        self.id
    }

    /// 返回物理区间。
    pub const fn range(&self) -> PhysRange {
        self.range
    }

    /// 返回页大小。
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// 返回用途。
    pub const fn kind(&self) -> CapacityKind {
        self.kind
    }

    /// 返回配额字节数。
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// 返回已分配字节数。
    pub const fn used(&self) -> u64 {
        self.used
    }

    fn coalesce(&mut self) {
        self.free.sort_unstable_by_key(|range| range.start().get());
        let mut merged: Vec<PhysRange> = Vec::with_capacity(self.free.len());
        for range in self.free.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.end().get() == range.start().get()
            {
                *last = PhysRange::new(last.start(), last.len().saturating_add(range.len()));
                continue;
            }
            merged.push(range);
        }
        self.free = merged;
    }
}

fn align_up(value: usize, alignment: usize) -> Option<u64> {
    let value = u64::try_from(value).ok()?;
    let alignment = alignment as u64;
    let aligned = value.checked_add(alignment.checked_sub(1)?)? / alignment * alignment;
    (aligned != 0).then_some(aligned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capacity_with_id(id: u64) -> Capacity {
        Capacity::new(
            CapacityId::new(id),
            PhysRange::new(PhysAddr::new(0x10_0000), 0x10_000),
            4096,
            CapacityKind::User,
            0,
        )
        .expect("Capacity")
    }

    fn capacity() -> Capacity {
        capacity_with_id(1)
    }

    #[test]
    fn allocates_and_reuses_frames() {
        let mut capacity = capacity();
        let first = capacity.allocate(4096).expect("第一页");
        assert_eq!(first.phys(), PhysAddr::new(0x10_0000));
        assert_eq!(first.len(), 4096);
        assert_eq!(capacity.used(), 4096);

        let second = capacity.allocate(8192).expect("第二段");
        assert_eq!(second.phys(), PhysAddr::new(0x10_1000));
        assert_eq!(second.len(), 8192);

        capacity.free(first).expect("归还");
        assert_eq!(capacity.used(), 8192);
        let reused = capacity.allocate(4096).expect("复用");
        assert_eq!(reused.phys(), PhysAddr::new(0x10_0000));
    }

    #[test]
    fn enforces_limit_and_page_size() {
        let mut capacity = Capacity::new(
            CapacityId::new(2),
            PhysRange::new(PhysAddr::new(0x20_0000), 0x3000),
            4096,
            CapacityKind::System,
            0,
        )
        .expect("Capacity");
        assert_eq!(
            capacity.allocate(0x4000).err(),
            Some(HmpError::OutOfCapacity)
        );
        assert_eq!(capacity.allocate(1).expect("对齐").len(), 4096);
    }

    #[test]
    fn rejects_foreign_and_pinned_frames() {
        let mut first = capacity();
        let mut second = capacity_with_id(2);
        let frame = first.allocate(4096).expect("页");
        assert_eq!(second.free(frame).err(), Some(HmpError::WrongCapability));

        let mut frame = first.allocate(4096).expect("页");
        frame.pin();
        assert_eq!(first.free(frame).err(), Some(HmpError::PermissionDenied));
    }

    #[test]
    fn rejects_shared_frame_until_last_reference() {
        let mut capacity = capacity();
        let mut frame = capacity.allocate(4096).expect("页");
        assert_eq!(frame.share(), Ok(2));
        assert_eq!(capacity.free(frame).err(), Some(HmpError::PermissionDenied));
    }

    #[test]
    fn splits_tail_into_child_capacity() {
        let mut parent = capacity();
        let mut child = parent
            .split(CapacityId::new(2), 0x4000, CapacityKind::User)
            .expect("子 Capacity");
        assert_eq!(
            child.range(),
            PhysRange::new(PhysAddr::new(0x10_0000), 0x4000)
        );
        assert_eq!(child.limit(), 0x4000);

        let child_frame = child.allocate(4096).expect("子分配");
        assert_eq!(child_frame.phys(), PhysAddr::new(0x10_0000));
        let parent_frame = parent.allocate(4096).expect("父分配");
        assert_eq!(parent_frame.phys(), PhysAddr::new(0x10_4000));
        assert_eq!(
            parent
                .split(CapacityId::new(3), 0x1_0000, CapacityKind::User)
                .err(),
            Some(HmpError::OutOfCapacity)
        );
    }
}
