//! 内核对象：对象头与负载。

use alloc::sync::Arc;

use hk_hal::DeviceGrant;
use hk_handle::{Object, ObjectHeader, ObjectKind};
use hk_ipc::{Endpoint, Notification};
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
}

impl Object for KernelObject {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
}
