//! 服务表：记录已加载服务的状态与退出码。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// 服务状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    /// 正在运行。
    Running,
    /// 已退出。
    Exited,
}

impl ServiceState {
    /// 返回状态名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
        }
    }
}

/// 服务条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// 服务名称。
    pub name: String,
    /// 当前状态。
    pub state: ServiceState,
    /// 退出码。
    pub exit_code: Option<u64>,
}

/// 服务表。
#[derive(Default)]
pub struct ServiceTable {
    services: Vec<Service>,
}

impl ServiceTable {
    /// 构造空服务表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记服务。
    pub fn register(&mut self, name: &str) {
        self.services.push(Service {
            name: name.to_string(),
            state: ServiceState::Running,
            exit_code: None,
        });
    }

    /// 标记服务退出。
    pub fn mark_exited(&mut self, name: &str, code: u64) {
        if let Some(service) = self
            .services
            .iter_mut()
            .find(|service| service.name == name)
        {
            service.state = ServiceState::Exited;
            service.exit_code = Some(code);
        }
    }

    /// 返回全部服务。
    pub fn services(&self) -> &[Service] {
        &self.services
    }

    /// 返回服务数量。
    pub fn len(&self) -> usize {
        self.services.len()
    }

    /// 判断服务表是否为空。
    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }
}
