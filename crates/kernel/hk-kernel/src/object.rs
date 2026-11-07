//! 内核对象：对象头与负载。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use hk_device::DeviceGrant;
use hk_hal::PhysRange;
use hk_handle::{Object, ObjectHeader, ObjectId, ObjectKind};
use hk_ipc::{Endpoint, Notification};
use hk_mem::hmp::FrameLease;
use hk_mem::hmp::{Capability, Capacity, Ledger, Region};
use spin::Mutex;

/// 内核对象的负载。
pub enum ObjectPayload {
    /// 同步 IPC 端点。
    Endpoint(Endpoint),
    /// 异步事件通知。
    Notification(Notification),
    /// 设备授权。
    DeviceGrant(DeviceGrant),
    /// 设备授权工厂。
    DeviceGrantFactory,
    /// 物理容量能力。
    Capacity(Mutex<Capacity>),
    /// 类型化能力。
    Capability {
        /// 能力描述。
        capability: Capability,
        /// 常驻物理页。
        backing: Option<FrameLease>,
        /// 缺页/回写 Provider 端点。
        provider: Mutex<Option<ObjectId>>,
        /// 按需装入的页租约；常驻能力为空。
        pages: Mutex<Vec<Option<FrameLease>>>,
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
            Self::Notification(_) => ObjectKind::Notification,
            Self::DeviceGrant(_) => ObjectKind::DeviceGrant,
            Self::DeviceGrantFactory => ObjectKind::DeviceGrantFactory,
            Self::Capacity(_) => ObjectKind::Capacity,
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

    /// 返回物理容量能力。
    pub fn capacity(&self) -> Option<&Mutex<Capacity>> {
        match &self.payload {
            ObjectPayload::Capacity(capacity) => Some(capacity),
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

    /// 返回类型化能力的常驻物理区间。
    pub fn capability_backing(&self) -> Option<PhysRange> {
        match &self.payload {
            ObjectPayload::Capability { backing, .. } => backing
                .as_ref()
                .map(|backing| PhysRange::new(backing.phys(), backing.len() as u64)),
            _ => None,
        }
    }

    /// 返回能力内的按需页租约。
    pub fn capability_page(&self, index: usize) -> Option<PhysRange> {
        match &self.payload {
            ObjectPayload::Capability { pages, .. } => pages
                .lock()
                .get(index)?
                .as_ref()
                .map(|lease| PhysRange::new(lease.phys(), lease.len() as u64)),
            _ => None,
        }
    }

    /// 装入能力内的一页。
    pub fn set_capability_page(&self, index: usize, lease: FrameLease) -> bool {
        if let ObjectPayload::Capability { pages, .. } = &self.payload {
            let mut pages = pages.lock();
            if let Some(slot) = pages.get_mut(index) {
                *slot = Some(lease);
                return true;
            }
        }
        false
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
}
