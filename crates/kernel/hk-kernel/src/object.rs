//! 内核对象：对象头与负载。

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use hk_device::DeviceGrant;
use hk_hal::PhysRange;
use hk_handle::{Object, ObjectHeader, ObjectId, ObjectKind};
use hk_ipc::{Endpoint, Notification};
use hk_mem::hmp::{Arena, Capability, Capacity, Frame, FrameBacking, Ledger, OwnedFrame, Region};
use spin::Mutex;

use crate::task::Task;

/// 内核对象的负载。
pub enum ObjectPayload {
    /// 同步 IPC 端点。
    Endpoint(Endpoint),
    /// 调度主体。
    Task(Arc<Mutex<Task>>),
    /// 时间契约。
    Cadence(Arc<Mutex<hk_cadence::Cadence>>),
    /// 调度计量。
    Meter(Arc<Mutex<hk_cadence::Meter>>),
    /// 异步事件通知。
    Notification(Notification),
    /// 设备授权。
    DeviceGrant(DeviceGrant),
    /// 设备授权工厂。
    DeviceGrantFactory,
    /// 任务内存根。
    Arena(Mutex<Arena>),
    /// 物理容量能力。
    Capacity(Arc<Mutex<Capacity>>),
    /// 物理页。
    Frame {
        /// 页描述；归还 Capacity 后为空。
        frame: Mutex<Option<Frame>>,
        /// Frame 归属的 Capacity；关闭最后一个句柄时自动归还。
        capacity: Option<Arc<Mutex<Capacity>>>,
    },
    /// 类型化能力。
    Capability {
        /// 能力描述。
        capability: Capability,
        /// 常驻物理页。
        backing: Option<FrameBacking>,
        /// 缺页/回写 Provider 端点。
        provider: Mutex<Option<ObjectId>>,
        /// 按需装入的页；只保存已装入页。
        pages: Mutex<BTreeMap<usize, FrameBacking>>,
        /// 是否固定驻留。
        pinned: AtomicBool,
        /// 是否正在 scanout 驻留。
        scanout: AtomicBool,
        /// Revision 版本。
        version: AtomicU64,
    },
    /// 虚拟映射。
    Region(Mutex<Region>),
    /// 配额与记账。
    Ledger(Mutex<Ledger>),
}

impl ObjectPayload {
    /// 返回负载对应的对象类别。
    pub const fn kind(&self) -> ObjectKind {
        match self {
            Self::Endpoint(_) => ObjectKind::Endpoint,
            Self::Task(_) => ObjectKind::Task,
            Self::Cadence(_) => ObjectKind::Cadence,
            Self::Meter(_) => ObjectKind::Meter,
            Self::Notification(_) => ObjectKind::Notification,
            Self::DeviceGrant(_) => ObjectKind::DeviceGrant,
            Self::DeviceGrantFactory => ObjectKind::DeviceGrantFactory,
            Self::Arena(_) => ObjectKind::Arena,
            Self::Capacity(_) => ObjectKind::Capacity,
            Self::Frame { .. } => ObjectKind::Frame,
            Self::Capability { .. } => ObjectKind::Capability,
            Self::Region(_) => ObjectKind::Region,
            Self::Ledger(_) => ObjectKind::Ledger,
        }
    }
}

/// 内核对象：对象头与负载。
///
/// 句柄持有 `Arc<KernelObject>`，最后一个句柄关闭时负载随之析构。
pub struct KernelObject {
    header: Arc<ObjectHeader>,
    payload: ObjectPayload,
}

impl KernelObject {
    /// 以负载创建对象并分配对象头。
    pub fn new(payload: ObjectPayload) -> Self {
        let header = ObjectHeader::new(payload.kind());
        Self { header, payload }
    }

    /// 返回对象类别。
    pub fn kind(&self) -> ObjectKind {
        self.payload.kind()
    }

    /// 返回端点负载。
    pub fn endpoint(&self) -> Option<&Endpoint> {
        match &self.payload {
            ObjectPayload::Endpoint(endpoint) => Some(endpoint),
            _ => None,
        }
    }

    /// 返回调度主体。
    pub fn task(&self) -> Option<&Arc<Mutex<Task>>> {
        match &self.payload {
            ObjectPayload::Task(task) => Some(task),
            _ => None,
        }
    }

    /// 返回时间契约。
    pub fn cadence(&self) -> Option<&Arc<Mutex<hk_cadence::Cadence>>> {
        match &self.payload {
            ObjectPayload::Cadence(cadence) => Some(cadence),
            _ => None,
        }
    }

    /// 返回调度计量。
    pub fn meter(&self) -> Option<&Arc<Mutex<hk_cadence::Meter>>> {
        match &self.payload {
            ObjectPayload::Meter(meter) => Some(meter),
            _ => None,
        }
    }

    /// 返回通知负载。
    pub fn notification(&self) -> Option<&Notification> {
        match &self.payload {
            ObjectPayload::Notification(notification) => Some(notification),
            _ => None,
        }
    }

    /// 返回设备授权负载。
    pub fn device_grant(&self) -> Option<&DeviceGrant> {
        match &self.payload {
            ObjectPayload::DeviceGrant(grant) => Some(grant),
            _ => None,
        }
    }

    /// 返回任务内存根。
    pub fn arena(&self) -> Option<&Mutex<Arena>> {
        match &self.payload {
            ObjectPayload::Arena(arena) => Some(arena),
            _ => None,
        }
    }

    /// 返回物理容量能力。
    pub fn capacity(&self) -> Option<&Arc<Mutex<Capacity>>> {
        match &self.payload {
            ObjectPayload::Capacity(capacity) => Some(capacity),
            _ => None,
        }
    }

    /// 返回物理页。
    pub fn frame(&self) -> Option<&Mutex<Option<Frame>>> {
        match &self.payload {
            ObjectPayload::Frame { frame, .. } => Some(frame),
            _ => None,
        }
    }

    /// 返回物理页归属的 Capacity；导入的区间为 `None`。
    pub fn frame_capacity(&self) -> Option<&Arc<Mutex<Capacity>>> {
        match &self.payload {
            ObjectPayload::Frame { capacity, .. } => capacity.as_ref(),
            _ => None,
        }
    }

    /// 取出物理页；取出后 Frame 对象保持为空。
    pub fn take_frame(&self) -> Option<Frame> {
        match &self.payload {
            ObjectPayload::Frame { frame, .. } => frame.lock().take(),
            _ => None,
        }
    }

    /// 返回类型化能力。
    pub fn capability(&self) -> Option<&Capability> {
        match &self.payload {
            ObjectPayload::Capability { capability, .. } => Some(capability),
            _ => None,
        }
    }

    /// 返回类型化能力的常驻字节数：后备区间加已装入页。
    pub fn capability_resident(&self) -> u64 {
        let ObjectPayload::Capability { backing, pages, .. } = &self.payload else {
            return 0;
        };
        let mut total = backing.as_ref().map_or(0, |backing| backing.range().len());
        total = total.saturating_add(pages.lock().len() as u64 * hk_hal::PAGE_SIZE as u64);
        total
    }

    /// 返回类型化能力的常驻物理区间。
    pub fn capability_backing(&self) -> Option<PhysRange> {
        match &self.payload {
            ObjectPayload::Capability { backing, .. } => backing.as_ref().map(FrameBacking::range),
            _ => None,
        }
    }

    /// 返回能力内的按需页。
    pub fn capability_page(&self, index: usize) -> Option<PhysRange> {
        match &self.payload {
            ObjectPayload::Capability { pages, .. } => {
                pages.lock().get(&index).map(FrameBacking::range)
            }
            _ => None,
        }
    }

    /// 取出能力内已装入的一页。
    pub fn take_capability_page(&self, index: usize) -> Option<FrameBacking> {
        match &self.payload {
            ObjectPayload::Capability { pages, .. } => pages.lock().remove(&index),
            _ => None,
        }
    }

    /// 装入能力内的一页。
    pub fn set_capability_page(&self, index: usize, page: FrameBacking) -> bool {
        if let ObjectPayload::Capability {
            capability,
            backing,
            pages,
            ..
        } = &self.payload
        {
            if backing.is_some() {
                return false;
            }
            let count = (capability.size() / capability.page_size() as u64) as usize;
            if index >= count {
                return false;
            }
            pages.lock().insert(index, page);
            return true;
        }
        false
    }

    /// 取出 Frame 及其 Capacity 归属。
    pub fn take_owned_frame(&self) -> Option<OwnedFrame> {
        let ObjectPayload::Frame { frame, capacity } = &self.payload else {
            return None;
        };
        let capacity = capacity.as_ref()?;
        let frame = frame.lock().take()?;
        Some(OwnedFrame::new(frame, Arc::clone(capacity)))
    }

    /// 返回类型化能力绑定的 Provider 端点。
    pub fn capability_provider(&self) -> Option<ObjectId> {
        match &self.payload {
            ObjectPayload::Capability { provider, .. } => *provider.lock(),
            _ => None,
        }
    }

    /// 绑定类型化能力的 Provider 端点。
    pub fn set_capability_provider(&self, endpoint: ObjectId) {
        if let ObjectPayload::Capability { provider, .. } = &self.payload {
            *provider.lock() = Some(endpoint);
        }
    }

    /// 判断类型化能力是否固定驻留。
    pub fn capability_pinned(&self) -> bool {
        match &self.payload {
            ObjectPayload::Capability { pinned, .. } => pinned.load(Ordering::Acquire),
            _ => false,
        }
    }

    /// 设置类型化能力的固定驻留状态。
    pub fn set_capability_pinned(&self, pinned: bool) {
        if let ObjectPayload::Capability { pinned: state, .. } = &self.payload {
            state.store(pinned, Ordering::Release);
        }
    }

    /// 判断类型化能力是否处于 scanout 驻留。
    pub fn capability_scanout(&self) -> bool {
        match &self.payload {
            ObjectPayload::Capability { scanout, .. } => scanout.load(Ordering::Acquire),
            _ => false,
        }
    }

    /// 设置类型化能力的 scanout 驻留状态。
    pub fn set_capability_scanout(&self, scanout: bool) {
        if let ObjectPayload::Capability { scanout: state, .. } = &self.payload {
            state.store(scanout, Ordering::Release);
        }
    }

    /// 推进 Revision 版本并返回新版本。
    pub fn commit_capability(&self) -> Option<u64> {
        match &self.payload {
            ObjectPayload::Capability { version, .. } => {
                Some(version.fetch_add(1, Ordering::AcqRel) + 1)
            }
            _ => None,
        }
    }

    /// 返回虚拟映射。
    pub fn region(&self) -> Option<&Mutex<Region>> {
        match &self.payload {
            ObjectPayload::Region(region) => Some(region),
            _ => None,
        }
    }

    /// 返回配额与记账。
    pub fn ledger(&self) -> Option<&Mutex<Ledger>> {
        match &self.payload {
            ObjectPayload::Ledger(ledger) => Some(ledger),
            _ => None,
        }
    }
}

impl Object for KernelObject {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn core::any::Any + Send + Sync> {
        self
    }
}

impl Drop for KernelObject {
    fn drop(&mut self) {
        let ObjectPayload::Frame { frame, capacity } = &self.payload else {
            return;
        };
        let Some(capacity) = capacity else {
            return;
        };
        let mut capacity = capacity.lock();
        let mut frame = frame.lock();
        if let Some(frame) = frame.as_mut() {
            // 最后一个句柄销毁时，pin 与残余 share 都不再能阻止物理页回收。
            frame.unpin();
            frame.clear_refs();
        }
        let _ = capacity.free(&mut frame);
    }
}
