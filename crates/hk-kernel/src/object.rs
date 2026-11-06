//! 内核对象：对象头与负载。

use alloc::sync::Arc;

use hk_hal::{DeviceGrant, PhysRange};
use hk_handle::{Object, ObjectHeader, ObjectKind};
use hk_ipc::{Endpoint, Notification};
use hk_mem::hmp::{Capability, Capacity, Ledger, Region};
use hk_mem::{Frame, Untyped, VSpace};
use spin::Mutex;

/// 内核对象的负载。
pub enum ObjectPayload {
    /// 地址空间。
    VSpace(VSpace),
    /// 同步 IPC 端点。
    Endpoint(Endpoint),
    /// 异步事件通知。
    Notification(Notification),
    /// 物理页帧。
    Frame(Frame),
    /// 未类型化内存。
    Untyped(Mutex<Untyped>),
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
        backing: Option<Frame>,
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
            Self::VSpace(_) => ObjectKind::VSpace,
            Self::Endpoint(_) => ObjectKind::Endpoint,
            Self::Notification(_) => ObjectKind::Notification,
            Self::Frame(_) => ObjectKind::Frame,
            Self::Untyped(_) => ObjectKind::Untyped,
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

    /// 以已有对象头创建对象，用于页帧等自带头的负载。
    pub fn with_header(header: Arc<ObjectHeader>, payload: ObjectPayload) -> Self {
        Self { header, payload }
    }

    /// 返回对象类别。
    pub fn kind(&self) -> ObjectKind {
        self.payload.kind()
    }

    /// 返回地址空间负载。
    pub fn vspace(&self) -> Option<&VSpace> {
        match &self.payload {
            ObjectPayload::VSpace(vspace) => Some(vspace),
            _ => None,
        }
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

    /// 返回页帧负载。
    pub fn frame(&self) -> Option<&Frame> {
        match &self.payload {
            ObjectPayload::Frame(frame) => Some(frame),
            _ => None,
        }
    }

    /// 返回未类型化内存负载。
    pub fn untyped(&self) -> Option<&Mutex<Untyped>> {
        match &self.payload {
            ObjectPayload::Untyped(untyped) => Some(untyped),
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
                .map(|frame| PhysRange::new(frame.phys(), frame.len() as u64)),
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
