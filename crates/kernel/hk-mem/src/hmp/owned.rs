//! 由 Capacity 拥有的 Frame 与能力后备。

use alloc::sync::Arc;

use hk_hal::PhysRange;
use spin::Mutex;

use super::{Capacity, Frame, FrameLease, HmpError};

/// 从 Capacity 分配、随对象生命周期归还的 Frame。
pub struct OwnedFrame {
    frame: Option<Frame>,
    capacity: Arc<Mutex<Capacity>>,
}

impl OwnedFrame {
    /// 从 Capacity 原子分配并建立 RAII 持有关系。
    ///
    /// 分配成功后立即由 `OwnedFrame` 接管；调用方在构造返回对象时发生
    /// 错误，析构路径仍会把物理页归还 Capacity。
    pub fn allocate(capacity: &Arc<Mutex<Capacity>>, len: usize) -> Result<Self, HmpError> {
        let frame = capacity.lock().allocate(len)?;
        Ok(Self::new(frame, Arc::clone(capacity)))
    }

    /// 按 NUMA 节点偏好原子分配并建立 RAII 持有关系。
    pub fn allocate_on(
        capacity: &Arc<Mutex<Capacity>>,
        len: usize,
        node: Option<u8>,
    ) -> Result<Self, HmpError> {
        let frame = capacity.lock().allocate_on(len, node)?;
        Ok(Self::new(frame, Arc::clone(capacity)))
    }

    /// 以 Frame 与所属 Capacity 创建持有关系。
    pub fn new(frame: Frame, capacity: Arc<Mutex<Capacity>>) -> Self {
        // 所有权转移到对象后，页表拆除不得再归还这段物理区间。
        if let Some(backend) = super::backend::physical_backend() {
            backend.mark_object_owned(hk_hal::PhysRange::new(frame.phys(), frame.len() as u64));
        }
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

    /// 返回帧所属 Capacity，供迁移到其他节点时从同一容量分配。
    pub fn capacity(&self) -> &Arc<Mutex<Capacity>> {
        &self.capacity
    }
    /// 取出内部 Frame，供失败路径把所有权放回原对象。
    pub fn into_frame(mut self) -> Frame {
        self.frame
            .take()
            .expect("OwnedFrame 在构造后必须持有 Frame")
    }
}

impl Drop for OwnedFrame {
    fn drop(&mut self) {
        let mut frame = self.frame.take();
        // 映射转移不延续 pin（见 `take_owned_frame`）；仍带 pin 说明调用方破坏
        // 了不变量，这里防御性解除，避免把可恢复的记账错误升级成内核终止。
        if let Some(frame) = frame.as_mut() {
            frame.unpin();
        }
        // 归还失败（归属不匹配、后端缺失）只会泄漏一段物理页：记录价值低于
        // 在析构路径终止内核，因此忽略返回值。
        let _ = self.capacity.lock().free(&mut frame);
    }
}

/// 类型化能力与 Provider 页的物理后备。
pub enum FrameBacking {
    /// 来自 Capacity 的 Frame，随后备释放归还 Capacity。
    Owned(OwnedFrame),
    /// 设备或外部区间导入，不归还 Capacity。
    Imported(FrameLease),
}

impl FrameBacking {
    /// 返回起始物理地址。
    pub fn phys(&self) -> hk_hal::PhysAddr {
        match self {
            Self::Owned(frame) => frame.phys(),
            Self::Imported(lease) => lease.phys(),
        }
    }

    /// 返回来源 Capacity；导入区间没有容量。
    pub fn capacity(&self) -> Option<&Arc<Mutex<Capacity>>> {
        match self {
            Self::Owned(frame) => Some(frame.capacity()),
            Self::Imported(_) => None,
        }
    }

    /// 返回字节长度。
    pub fn len(&self) -> usize {
        match self {
            Self::Owned(frame) => frame.len(),
            Self::Imported(lease) => lease.len(),
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
