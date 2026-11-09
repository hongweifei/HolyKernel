//! 服务目录：镜像构建、启动白名单、桌面排序与默认 Cadence 的单一清单。
//!
//! 新增服务时只修改 [`SERVICES`]；构建工具、Root Server 与平台装载从同一
//! 清单读取名称、启动标记、桌面排序与默认调度契约。

#![no_std]

use core::iter::Iterator;

use hk_cadence::{CadenceClass, CadenceSpec, Lending, millis};

/// 服务条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    /// 服务镜像名。
    pub name: &'static str,
    /// 是否在默认启动白名单中。
    pub boot: bool,
    /// 桌面模式排序；数值小的先启动。
    pub desktop_order: u8,
    /// 默认 Cadence；Root/Tempo 按此授予初始契约。
    pub cadence: CadenceSpec,
}

/// 交互类服务：显示、输入与终端。
const REALTIME: CadenceSpec = CadenceSpec::new(CadenceClass::Realtime, 4096)
    .with_budget(millis(3), millis(16))
    .with_deadline(millis(8))
    .with_latency(millis(8))
    .with_lending(Lending::caller_pays());

/// 高份额低延迟服务：文件系统、块与驱动。
const LATENCY_HIGH: CadenceSpec = CadenceSpec::new(CadenceClass::Latency, 3072)
    .with_latency(millis(4))
    .with_lending(Lending::caller_pays());

/// 普通低延迟服务：网络、检查点、调试与桌面原生服务。
const LATENCY: CadenceSpec = CadenceSpec::new(CadenceClass::Latency, 2048)
    .with_latency(millis(4))
    .with_lending(Lending::caller_pays());

/// 后台吞吐服务：编译、索引与自检。
const THROUGHPUT: CadenceSpec =
    CadenceSpec::new(CadenceClass::Throughput, 1024).with_lending(Lending::caller_pays());

/// Tempo 的初始策略池：实时类、带硬预算，子契约在其中收窄。
const TEMPO_POOL: CadenceSpec = CadenceSpec::new(CadenceClass::Realtime, 4096)
    .with_budget(millis(8), millis(16))
    .with_latency(millis(2))
    .with_lending(Lending::caller_pays());

/// 未登记服务的默认契约。
pub const DEFAULT_CADENCE: CadenceSpec = LATENCY;

macro_rules! services {
    ($(($name:literal, $boot:literal, $desktop:literal, $cadence:expr)),* $(,)?) => {
        /// 全部服务；顺序即镜像构建与默认装载顺序。
        pub const SERVICES: &[Service] = &[
            $(Service {
                name: $name,
                boot: $boot,
                desktop_order: $desktop,
                cadence: $cadence,
            }),*
        ];

        /// 全部服务名，供构建工具遍历。
        pub const SERVERS: &[&str] = &[$($name),*];
    };
}

services![
    ("hk-server-demo", true, 9, THROUGHPUT),
    ("hk-server-echo", true, 9, THROUGHPUT),
    ("hk-server-device", true, 1, LATENCY_HIGH),
    ("hk-server-spin", true, 9, THROUGHPUT),
    ("hk-server-virtio", true, 3, LATENCY_HIGH),
    ("hk-server-memory", true, 9, LATENCY_HIGH),
    ("hk-server-ext4", true, 4, LATENCY_HIGH),
    ("hk-server-holyfs", true, 4, LATENCY_HIGH),
    ("hk-server-vfs", true, 5, LATENCY_HIGH),
    ("hk-server-path", true, 6, LATENCY_HIGH),
    ("hk-server-fault", false, 9, THROUGHPUT),
    ("hk-server-fork", false, 9, THROUGHPUT),
    ("hk-server-latency", false, 9, THROUGHPUT),
    ("hk-server-block", true, 9, LATENCY_HIGH),
    ("hk-server-virtio-net", true, 9, LATENCY_HIGH),
    ("hk-server-net", true, 9, LATENCY),
    ("hk-server-netstat", false, 9, THROUGHPUT),
    ("hk-server-netsock", true, 9, LATENCY),
    ("hk-server-console", true, 8, REALTIME),
    ("hk-server-console-test", true, 9, THROUGHPUT),
    ("hk-server-shell", true, 9, LATENCY),
    ("hk-server-tempo", true, 8, TEMPO_POOL),
    ("hk-server-display", true, 0, REALTIME),
    ("hk-server-display-test", true, 9, THROUGHPUT),
    ("hk-server-terminal-test", true, 9, THROUGHPUT),
    ("hk-server-input", true, 2, REALTIME),
    ("hk-server-pointer-test", true, 9, THROUGHPUT),
    ("hk-server-holydoc", true, 7, LATENCY),
    ("hk-server-workbench", true, 7, LATENCY),
    ("hk-server-debug", true, 9, LATENCY),
    ("hk-server-taskman", true, 7, LATENCY),
    ("hk-server-dbg", false, 9, THROUGHPUT),
    ("hk-server-display-restart-test", true, 9, THROUGHPUT),
    ("hk-server-checkpoint", true, 9, LATENCY),
    ("hk-server-checkpoint-test", true, 9, THROUGHPUT),
    ("hk-server-memory-test", false, 9, THROUGHPUT),
    ("hk-server-tempo-test", false, 9, THROUGHPUT),
    ("hk-server-wakeup", false, 9, THROUGHPUT),
];

/// 按名称查找服务。
pub fn service(name: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|service| service.name == name)
}

/// 判断服务是否在默认启动白名单中。
pub fn boot_service(name: &str) -> bool {
    service(name).is_some_and(|service| service.boot)
}

/// 返回桌面模式排序键；未登记服务排在最后。
pub fn desktop_order(name: &str) -> u8 {
    service(name).map_or(9, |service| service.desktop_order)
}

/// 返回服务的默认 Cadence；未登记服务取 [`DEFAULT_CADENCE`]。
pub fn default_cadence(name: &str) -> CadenceSpec {
    service(name).map_or(DEFAULT_CADENCE, |service| service.cadence)
}

/// 判断服务是否持有调度策略授权；当前只有 Tempo。
pub fn policy_authority(name: &str) -> bool {
    name == "hk-server-tempo"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_services_default_to_realtime_interactive_contract() {
        for name in ["hk-server-display", "hk-server-input", "hk-server-console"] {
            let cadence = default_cadence(name);
            assert_eq!(cadence.class, CadenceClass::Realtime);
            assert!(cadence.budget.ticks() > 0);
            assert!(cadence.period.ticks() >= cadence.budget.ticks());
        }
    }

    #[test]
    fn filesystem_services_get_higher_share_than_background() {
        let filesystem = default_cadence("hk-server-vfs");
        let background = default_cadence("hk-server-demo");
        assert!(filesystem.weight > background.weight);
        assert_eq!(filesystem.class, CadenceClass::Latency);
        assert_eq!(background.class, CadenceClass::Throughput);
    }

    #[test]
    fn unknown_service_uses_default_cadence() {
        assert_eq!(default_cadence("hk-server-unknown"), DEFAULT_CADENCE);
        assert_eq!(desktop_order("hk-server-unknown"), 9);
    }
}
