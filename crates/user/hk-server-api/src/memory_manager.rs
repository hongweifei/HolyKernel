//! Memory Server 客户端：按类型申请能力并映射。

use crate::{
    CapabilityKind, MEMORY_ALLOC, MEMORY_ALLOCATED, MEMORY_FRAME, MEMORY_FRAME_ALLOCATED,
    close_handle, map_attach, recv_data_handle, send_data, service_endpoint,
};

/// 请求载荷：能力类型（1 字节）+ 标志（1 字节）+ 长度（8 字节小端）。
const REQUEST_BYTES: usize = 1 + 1 + 8;
/// 请求标志：按需装入的匿名能力（由 Memory Server 作为 Provider）。
const FLAG_LAZY: u8 = 1 << 0;

/// Memory Server 客户端。
pub struct MemoryClient {
    own_endpoint: u64,
    service: u64,
}

impl MemoryClient {
    /// 以本任务端点构造客户端。
    pub fn new(own_endpoint: u64) -> Self {
        Self {
            own_endpoint,
            service: service_endpoint("hk-server-memory"),
        }
    }

    /// 判断 Memory Server 是否可用。
    pub const fn available(&self) -> bool {
        self.service != 0
    }

    fn request(&self, kind: CapabilityKind, length: usize, lazy: bool) -> Option<u64> {
        if !self.available() || length == 0 {
            return None;
        }
        let mut request = [0_u8; REQUEST_BYTES];
        request[0] = kind.as_u8();
        request[1] = if lazy { FLAG_LAZY } else { 0 };
        request[2..].copy_from_slice(&(length as u64).to_le_bytes());
        if send_data(self.service, MEMORY_ALLOC, &request) != 0 {
            return None;
        }
        // 只取 Memory Server 的回复，避免把本端点上的其他消息当成分配结果。
        let mut payload = [0_u8; 0];
        let (label, word, _sender, _length, handle) =
            recv_data_handle(self.own_endpoint, &mut payload, self.service);
        (label == MEMORY_ALLOCATED && word != u64::MAX && handle != 0).then_some(handle)
    }

    /// 申请一段带常驻后备的能力。
    pub fn allocate(&self, kind: CapabilityKind, length: usize) -> Option<u64> {
        self.request(kind, length, false)
    }

    /// 申请一段按需装入的匿名能力。
    pub fn allocate_lazy(&self, length: usize) -> Option<u64> {
        self.request(CapabilityKind::Anonymous, length, true)
    }

    /// 申请并映射带后备的能力，返回 `(capability, region, virt)`。
    pub fn map(&self, kind: CapabilityKind, length: usize) -> Option<(u64, u64, u64)> {
        let capability = self.allocate(kind, length)?;
        let Some((region, virt)) = map_attach(capability) else {
            let _ = close_handle(capability);
            return None;
        };
        Some((capability, region, virt))
    }

    /// 申请并映射按需装入的匿名能力，返回 `(capability, region, virt)`。
    pub fn map_lazy(&self, length: usize) -> Option<(u64, u64, u64)> {
        let capability = self.allocate_lazy(length)?;
        let Some((region, virt)) = crate::map_lazy(capability, length) else {
            let _ = close_handle(capability);
            return None;
        };
        Some((capability, region, virt))
    }

    /// 从 Memory Server 的 Capacity 申请一个 Frame。
    pub fn frame(&self, length: usize) -> Option<u64> {
        if !self.available() || length == 0 {
            return None;
        }
        let request = (length as u64).to_le_bytes();
        if send_data(self.service, MEMORY_FRAME, &request) != 0 {
            return None;
        }
        let mut payload = [0_u8; 0];
        let (label, word, _sender, _length, handle) =
            recv_data_handle(self.own_endpoint, &mut payload, self.service);
        (label == MEMORY_FRAME_ALLOCATED && word != u64::MAX && handle != 0).then_some(handle)
    }
}
