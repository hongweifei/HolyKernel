//! 由 Capacity 拥有的 Frame 与能力后备。

use alloc::sync::Arc;
use core::ops::Deref;
use core::sync::atomic::{AtomicU32, Ordering};

use hk_hal::{PAGE_SIZE, PhysRange};
use spin::Mutex;

use super::{Capacity, Frame, FrameLease};

/// 从 Capacity 分配、随对象生命周期归还的 Frame。
pub struct OwnedFrame {
    frame: Option<Frame>,
    capacity: Arc<Mutex<Capacity>>,
}

impl OwnedFrame {
    /// 以 Frame 与所属 Capacity 创建持有关系。
    pub fn new(frame: Frame, capacity: Arc<Mutex<Capacity>>) -> Self {
        Self {
            frame: Some(frame),
            capacity,
        }
    }

    /// 返回起始物理地址。
    pub fn phys(&self) -> hk_hal::PhysAddr {
        self.frame
            .as_ref()
            .map_or(hk_hal::PhysAddr::new(0), Frame::phys)
    }

    /// 返回字节长度。
    pub fn len(&self) -> usize {
        self.frame.as_ref().map_or(0, Frame::len)
    }

    /// 判断长度是否为零。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 返回页大小。
    pub fn page_size(&self) -> usize {
        self.frame.as_ref().map_or(0, Frame::page_size)
    }

    /// 返回物理区间。
    pub fn range(&self) -> PhysRange {
        PhysRange::new(self.phys(), self.len() as u64)
    }
}

impl Drop for OwnedFrame {
    fn drop(&mut self) {
        let mut frame = self.frame.take();
        let _ = self.capacity.lock().free(&mut frame);
    }
}

/// 可由多个句柄或映射共享的 Frame 单元格。
///
/// 句柄唯一且没有共享映射时，`SYS_FRAME_MAP` 把物理页移入能力后备；否则
/// 物理页留在本单元格里，映射只持有一份 [`SharedFrameRef`]。物理页在最后
/// 一个引用释放后才归还 Capacity，因此同一 Frame 的多个句柄可以各自映射。
pub struct SharedFrame {
    frame: Mutex<Option<Frame>>,
    capacity: Option<Arc<Mutex<Capacity>>>,
    shared: AtomicU32,
}

impl SharedFrame {
    /// 以 Frame 与可选 Capacity 归属创建共享单元格。
    pub fn new(frame: Frame, capacity: Option<Arc<Mutex<Capacity>>>) -> Self {
        Self {
            frame: Mutex::new(Some(frame)),
            capacity,
            shared: AtomicU32::new(0),
        }
    }

    /// 锁定物理页描述；取出后单元格保持为空。
    pub fn lock(&self) -> spin::MutexGuard<'_, Option<Frame>> {
        self.frame.lock()
    }

    /// 取出物理页描述。
    pub fn take(&self) -> Option<Frame> {
        self.frame.lock().take()
    }

    /// 返回所属 Capacity；导入区间为 `None`。
    pub fn capacity(&self) -> Option<&Arc<Mutex<Capacity>>> {
        self.capacity.as_ref()
    }

    /// 返回起始物理地址；物理页已移出时为 0。
    pub fn phys(&self) -> hk_hal::PhysAddr {
        self.frame
            .lock()
            .as_ref()
            .map_or(hk_hal::PhysAddr::new(0), Frame::phys)
    }

    /// 返回字节长度。
    pub fn len(&self) -> usize {
        self.frame.lock().as_ref().map_or(0, Frame::len)
    }

    /// 判断长度是否为零。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 返回页大小。
    pub fn page_size(&self) -> usize {
        self.frame.lock().as_ref().map_or(0, Frame::page_size)
    }

    /// 返回物理区间。
    pub fn range(&self) -> PhysRange {
        PhysRange::new(self.phys(), self.len() as u64)
    }

    /// 是否已有共享映射在引用本单元格。
    pub fn is_shared(&self) -> bool {
        self.shared.load(Ordering::Acquire) > 0
    }

    /// 建立一份共享映射引用。
    pub fn share(self: &Arc<Self>) -> SharedFrameRef {
        self.shared.fetch_add(1, Ordering::AcqRel);
        SharedFrameRef(Arc::clone(self))
    }
}

impl Drop for SharedFrame {
    fn drop(&mut self) {
        // 最后一个引用释放：固定与残余 share 不再能阻止物理页回收。
        let Some(capacity) = &self.capacity else {
            self.frame.lock().take();
            return;
        };
        let mut capacity = capacity.lock();
        let mut frame = self.frame.lock();
        if let Some(frame) = frame.as_mut() {
            frame.unpin();
            frame.clear_refs();
        }
        let _ = capacity.free(&mut frame);
    }
}

/// 一份共享 Frame 引用；释放时只减少共享计数。
pub struct SharedFrameRef(Arc<SharedFrame>);

impl Deref for SharedFrameRef {
    type Target = SharedFrame;

    fn deref(&self) -> &SharedFrame {
        &self.0
    }
}

impl Drop for SharedFrameRef {
    fn drop(&mut self) {
        self.0.shared.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 类型化能力与 Provider 页的物理后备。
pub enum FrameBacking {
    /// 来自 Capacity 的 Frame，随后备释放归还 Capacity。
    Owned(OwnedFrame),
    /// 与其它句柄共享的 Frame；物理页在最后一个引用释放后归还。
    Shared(SharedFrameRef),
    /// 设备或外部区间导入，不归还 Capacity。
    Imported(FrameLease),
}

impl FrameBacking {
    /// 返回起始物理地址。
    pub fn phys(&self) -> hk_hal::PhysAddr {
        match self {
            Self::Owned(frame) => frame.phys(),
            Self::Shared(frame) => frame.phys(),
            Self::Imported(lease) => lease.phys(),
        }
    }

    /// 返回字节长度。
    pub fn len(&self) -> usize {
        match self {
            Self::Owned(frame) => frame.len(),
            Self::Shared(frame) => frame.len(),
            Self::Imported(lease) => lease.len(),
        }
    }

    /// 返回页大小。
    pub fn page_size(&self) -> usize {
        match self {
            Self::Owned(frame) => frame.page_size(),
            Self::Shared(frame) => frame.page_size(),
            Self::Imported(_) => PAGE_SIZE,
        }
    }

    /// 判断长度是否为零。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 返回物理区间。
    pub fn range(&self) -> PhysRange {
        PhysRange::new(self.phys(), self.len() as u64)
    }
}
