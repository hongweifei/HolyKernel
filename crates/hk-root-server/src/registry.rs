//! 服务表：记录已加载服务的状态与退出码。
//!
//! 服务表是 Root Server 的策略数据，与具体平台和硬件无关。

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
pub struct ServiceRecord {
    /// 服务名称。
    pub name: String,
    /// 当前状态。
    pub state: ServiceState,
    /// 重启次数。
    pub restarts: u32,
    /// 退出码。
    pub exit_code: Option<u64>,
}

/// 服务表。
#[derive(Default)]
pub struct ServiceTable {
    services: Vec<ServiceRecord>,
}

impl ServiceTable {
    /// 构造空服务表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记服务。
    pub fn register(&mut self, name: &str) {
        self.services.push(ServiceRecord {
            name: name.to_string(),
            state: ServiceState::Running,
            restarts: 0,
            exit_code: None,
        });
    }

    /// 标记服务运行；服务不在表中时登记。
    pub fn mark_running(&mut self, name: &str) {
        if let Some(service) = self
            .services
            .iter_mut()
            .find(|service| service.name == name)
        {
            if service.state == ServiceState::Exited {
                service.restarts = service.restarts.saturating_add(1);
            }
            service.state = ServiceState::Running;
            service.exit_code = None;
        } else {
            self.register(name);
        }
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
    pub fn services(&self) -> &[ServiceRecord] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_service_lifecycle() {
        let mut table = ServiceTable::new();
        assert!(table.is_empty());

        table.register("clock");
        table.register("log");
        assert_eq!(table.len(), 2);
        assert_eq!(table.services()[0].state, ServiceState::Running);
        assert_eq!(table.services()[0].exit_code, None);

        table.mark_exited("clock", 7);
        assert_eq!(table.services()[0].state, ServiceState::Exited);
        assert_eq!(table.services()[0].exit_code, Some(7));
        assert_eq!(table.services()[1].state, ServiceState::Running);
    }

    #[test]
    fn ignores_unknown_service() {
        let mut table = ServiceTable::new();
        table.register("clock");
        table.mark_exited("missing", 1);
        assert_eq!(table.services()[0].state, ServiceState::Running);
    }

    #[test]
    fn restarts_service_record() {
        let mut table = ServiceTable::new();
        table.register("clock");
        table.mark_exited("clock", 3);
        table.mark_running("clock");
        assert_eq!(table.len(), 1);
        assert_eq!(table.services()[0].state, ServiceState::Running);
        assert_eq!(table.services()[0].restarts, 1);
        assert_eq!(table.services()[0].exit_code, None);

        table.mark_running("log");
        assert_eq!(table.len(), 2);
    }
}
