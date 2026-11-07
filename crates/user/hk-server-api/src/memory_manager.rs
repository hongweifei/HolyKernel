//! Memory Server 客户端：申请匿名能力并映射。

use crate::{MEMORY_ALLOC, MEMORY_ALLOCATED, map_lazy, recv_handle, send, service_endpoint};

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

    /// 申请一段按需装入的匿名能力。
    pub fn allocate(&self, length: usize) -> Option<u64> {
        if !self.available() || length == 0 || send(self.service, MEMORY_ALLOC, length as u64) != 0
        {
            return None;
        }
        let (label, word, _sender, handle) = recv_handle(self.own_endpoint);
        (label == MEMORY_ALLOCATED && word != u64::MAX && handle != 0).then_some(handle)
    }

    /// 申请并映射一段按需装入的匿名能力，返回 `(region, virt)`。
    pub fn map(&self, length: usize) -> Option<(u64, u64)> {
        let capability = self.allocate(length)?;
        map_lazy(capability, length)
    }
}
