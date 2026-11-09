//! 用户态运行时与 Root Server 启动原语。
//!
//! 按服务清单创建停泊的服务线程、端点与句柄，并维护用户任务视图、系统调用
//! 分派与 Arena 记账；平台只提供现场保存/恢复、硬件副作用与地址空间切换。

#![no_std]

extern crate alloc;

pub mod boot;
pub mod runtime;

pub use runtime::{
    Arena, FaultOutcome, Pending, RuntimeStats, Step, Switch, TaskInfo, UserPlatform, UserRuntime,
    UserTask,
};

use alloc::vec::Vec;

use hk_cadence::{CadenceClass, CadenceSpec, DEFAULT_SPEC, millis};
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_kernel::{Kernel, KernelError, TcbId};
use hk_mem::hmp::CapacityKind;

/// Root Server 的初始契约。
const ROOT_CADENCE: CadenceSpec =
    CadenceSpec::new(CadenceClass::Latency, 4096).with_latency(millis(1));

/// 初始 Capacity 授予方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityGrant {
    /// 从 System Capacity 尾部切出一段独立子容量。
    Split(CapacityKind, usize),
    /// 授予启动期建立的 System Capacity 共享视图。
    System,
}

/// 服务清单条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    /// 服务名称。
    pub name: &'static str,
    /// 是否允许通过设备授权工厂创建 DeviceGrant。
    pub device_authority: bool,
    /// 初始 Capacity 授予。
    pub capacity: Option<CapacityGrant>,
    /// 初始 Cadence 契约。
    pub cadence: CadenceSpec,
    /// 是否持有调度策略授权；由 Root 在启动时授予 Tempo。
    pub policy_authority: bool,
}

impl ServiceSpec {
    /// 构造普通服务清单条目。
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            device_authority: false,
            capacity: None,
            cadence: DEFAULT_SPEC,
            policy_authority: false,
        }
    }

    /// 构造具有设备授权创建能力的服务清单条目。
    pub const fn device(name: &'static str) -> Self {
        Self {
            name,
            device_authority: true,
            capacity: None,
            cadence: DEFAULT_SPEC,
            policy_authority: false,
        }
    }

    /// 声明初始 Capacity 授予：从 System Capacity 尾部切出独立子容量。
    pub const fn with_capacity(mut self, kind: CapacityKind, bytes: usize) -> Self {
        self.capacity = Some(CapacityGrant::Split(kind, bytes));
        self
    }

    /// 授予启动期建立的 System Capacity 共享视图。
    pub const fn with_system_capacity(mut self) -> Self {
        self.capacity = Some(CapacityGrant::System);
        self
    }

    /// 指定初始 Cadence 契约。
    pub const fn with_cadence(mut self, cadence: CadenceSpec) -> Self {
        self.cadence = cadence;
        self
    }

    /// 授予调度策略授权。
    pub const fn with_policy_authority(mut self) -> Self {
        self.policy_authority = true;
        self
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
        Self::boot_impl(kernel, specs, false)
    }

    /// 启动 Root Server，但服务线程保持停泊，等待调用方 attach 后激活。
    pub fn boot_parked(kernel: &Kernel, specs: &[ServiceSpec]) -> Result<Self, KernelError> {
        Self::boot_impl(kernel, specs, true)
    }

    fn boot_impl(
        kernel: &Kernel,
        specs: &[ServiceSpec],
        parked: bool,
    ) -> Result<Self, KernelError> {
        let spawn = |handles, cadence| {
            if parked {
                kernel.spawn_task_parked(handles, cadence)
            } else {
                kernel.spawn_task(handles, cadence)
            }
        };

        let root = spawn(HandleTable::with_default_quota(), ROOT_CADENCE)?;
        let mut services = Vec::with_capacity(specs.len());

        for spec in specs {
            let tcb = spawn(HandleTable::with_default_quota(), spec.cadence)?;
            if spec.policy_authority {
                let task = kernel.task_of(tcb).ok_or(KernelError::UnknownThread(tcb))?;
                kernel.set_policy_authority(task, true)?;
            }
            let endpoint =
                kernel.create_endpoint(root, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
            let endpoint_id = kernel.handle_id(root, endpoint)?;
            let handle = kernel.grant(
                tcb,
                endpoint_id,
                Rights::READ | Rights::WRITE | Rights::TRANSFER,
            )?;
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
        let client = kernel
            .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
            .expect("创建客户端任务");

        let handle = kernel
            .grant(
                client,
                service.endpoint,
                Rights::READ | Rights::WRITE | Rights::TRANSFER,
            )
            .expect("授予客户端句柄");
        assert_eq!(
            kernel.handle_id(client, handle).expect("解析句柄"),
            service.endpoint
        );
    }
}
