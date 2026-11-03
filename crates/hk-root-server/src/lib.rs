//! Root Server stub。
//!
//! 读取服务清单，为每个服务创建 TCB、端点与最小句柄集合，并把服务登记进服务表。
//! 服务监督与重启在 M3 接入；M1 只完成启动与句柄分配。

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_kernel::{Kernel, KernelError, Priority, TcbId};

/// 服务清单条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    /// 服务名称。
    pub name: &'static str,
}

impl ServiceSpec {
    /// 构造服务清单条目。
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

/// 已启动的服务。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    /// 服务名称。
    pub name: &'static str,
    /// 服务线程。
    pub tcb: TcbId,
    /// 服务端点对象。
    pub endpoint: ObjectId,
    /// 服务侧端点句柄，在服务自己的句柄表中有效。
    pub handle: Handle,
}

/// Root Server 实例。
pub struct RootServer {
    root: TcbId,
    services: Vec<Service>,
}

impl RootServer {
    /// 启动 Root Server 与清单中的全部服务。
    ///
    /// Root 线程使用最高优先级；每个服务获得独立句柄表、一个端点以及在自身表中
    /// 有效的端点句柄。
    pub fn boot(kernel: &Kernel, specs: &[ServiceSpec]) -> Result<Self, KernelError> {
        let root = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::HIGHEST);
        let mut services = Vec::with_capacity(specs.len());

        for spec in specs {
            let tcb = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
            let endpoint = kernel.create_endpoint(root, Rights::READ | Rights::WRITE)?;
            let endpoint_id = kernel.handle_id(root, endpoint)?;
            let handle = kernel.grant(tcb, endpoint_id, Rights::READ | Rights::WRITE)?;
            services.push(Service {
                name: spec.name,
                tcb,
                endpoint: endpoint_id,
                handle,
            });
        }

        Ok(Self { root, services })
    }

    /// 返回 Root 线程。
    pub fn root(&self) -> TcbId {
        self.root
    }

    /// 返回服务表。
    pub fn services(&self) -> &[Service] {
        &self.services
    }

    /// 按名称查询服务。
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|service| service.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boots_services_with_endpoints_and_handles() {
        let kernel = Kernel::new();
        let server = RootServer::boot(
            &kernel,
            &[ServiceSpec::new("clock"), ServiceSpec::new("log")],
        )
        .expect("启动 Root Server");

        assert_eq!(kernel.thread_count(), 3);
        assert_eq!(kernel.live_objects(), 2);
        assert_eq!(server.services().len(), 2);
        assert_eq!(
            server.service("clock").map(|service| service.name),
            Some("clock")
        );
        assert_eq!(server.service("missing"), None);
    }

    #[test]
    fn services_block_on_their_endpoint() {
        let kernel = Kernel::new();
        let server = RootServer::boot(&kernel, &[ServiceSpec::new("clock")]).expect("启动");
        let service = server.service("clock").expect("服务存在");

        assert_eq!(
            kernel.recv(service.tcb, service.handle).expect("接收"),
            None
        );
    }

    #[test]
    fn endpoint_can_be_granted_to_clients() {
        let kernel = Kernel::new();
        let server = RootServer::boot(&kernel, &[ServiceSpec::new("clock")]).expect("启动");
        let service = server.service("clock").expect("服务存在");
        let client = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);

        let handle = kernel
            .grant(client, service.endpoint, Rights::READ | Rights::WRITE)
            .expect("授予客户端句柄");
        assert_eq!(
            kernel.handle_id(client, handle).expect("解析句柄"),
            service.endpoint
        );
    }
}
