//! Root Server 服务管理：服务表、装载、监督与运行时宿主。
//!
//! 服务清单、端点与停泊线程由 [`hk_runtime`] 创建；本 crate 负责把镜像装载为
//! 用户现场、维护服务状态、按策略重启，并通过 [`RuntimeHost`] 驱动运行时。

#![no_std]

extern crate alloc;

pub mod debug;
pub mod host;
pub mod image;
pub mod loader;
mod registry;
pub mod service;

pub use hk_image::ServiceRole;
pub use hk_kernel::{TraceCategory, TraceEvent};
pub use hk_runtime::{
    CowOutcome, FaultOutcome, Pending, RootServer, RuntimeStats, Service, ServiceSpec, Step,
    Switch, TaskInfo, UserPlatform, UserRegionSpec, UserRuntime, UserTask,
};
pub use host::RuntimeHost;
pub use registry::{ServiceRecord, ServiceState, ServiceTable};
pub use service::RestartPolicy;
