//! 服务清单：默认 Cadence、重启策略常量与 [`SERVICES`] 表。
//!
//! 新增服务时只修改 [`SERVICES`]；构建工具、Root Server 与平台装载都从同一
//! 张表读取名称、启动标记、桌面排序、重启策略与 opt-in 标记。

use hk_cadence::{CadenceClass, CadenceSpec, Lending, millis};

use super::{RestartPolicy, Service};

const NEVER: RestartPolicy = RestartPolicy::Never;
const ON_FAILURE: RestartPolicy = RestartPolicy::OnFailure;
const ALWAYS: RestartPolicy = RestartPolicy::Always;

/// 交互类服务：显示、输入与终端。
const REALTIME: CadenceSpec = CadenceSpec::new(CadenceClass::Realtime, 4096)
    .with_budget(millis(2), millis(16))
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
    .with_budget(millis(4), millis(16))
    .with_deadline(millis(16))
    .with_latency(millis(2))
    .with_lending(Lending::caller_pays());

/// 未登记服务的默认契约。
pub const DEFAULT_CADENCE: CadenceSpec = LATENCY;

macro_rules! services {
    ($(($name:literal, $boot:literal, $desktop:literal, $cadence:expr, $restart:expr, $opt_in:expr)),* $(,)?) => {
        /// 全部服务；顺序即镜像构建、默认装载与 opt-in 追加顺序。
        pub const SERVICES: &[Service] = &[
            $(Service {
                name: $name,
                boot: $boot,
                desktop_order: $desktop,
                cadence: $cadence,
                restart: $restart,
                opt_in: $opt_in,
            }),*
        ];

        /// 全部服务名，供构建工具遍历。
        pub const SERVERS: &[&str] = &[$($name),*];
    };
}

services![
    ("hk-server-demo", true, 9, THROUGHPUT, NEVER, None),
    ("hk-server-echo", true, 9, THROUGHPUT, NEVER, None),
    ("hk-server-device", true, 1, LATENCY_HIGH, ALWAYS, None),
    ("hk-server-spin", true, 9, THROUGHPUT, NEVER, None),
    ("hk-server-virtio", true, 3, LATENCY_HIGH, ON_FAILURE, None),
    ("hk-server-virtio-sound", true, 3, LATENCY, NEVER, None),
    ("hk-server-ahci", true, 3, LATENCY_HIGH, NEVER, None),
    ("hk-server-nvme", true, 3, LATENCY_HIGH, NEVER, None),
    ("hk-server-memory", true, 9, LATENCY_HIGH, NEVER, None),
    ("hk-server-ext4", true, 4, LATENCY_HIGH, ON_FAILURE, None),
    ("hk-server-holyfs", true, 4, LATENCY_HIGH, NEVER, None),
    ("hk-server-vfs", true, 5, LATENCY_HIGH, NEVER, None),
    ("hk-server-path", true, 6, LATENCY_HIGH, NEVER, None),
    ("hk-server-fault", false, 9, THROUGHPUT, NEVER, None),
    ("hk-server-fork", false, 9, THROUGHPUT, NEVER, None),
    ("hk-server-latency", false, 9, THROUGHPUT, NEVER, None),
    ("hk-server-block", true, 9, LATENCY_HIGH, NEVER, None),
    ("hk-server-virtio-net", true, 9, LATENCY_HIGH, ALWAYS, None),
    ("hk-server-net", true, 9, LATENCY, NEVER, None),
    ("hk-client-netstat", false, 9, THROUGHPUT, NEVER, None),
    (
        "hk-client-netsock",
        false,
        9,
        LATENCY,
        NEVER,
        Some("net-test")
    ),
    ("hk-server-console", true, 8, REALTIME, NEVER, None),
    (
        "hk-client-console-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("console-test")
    ),
    (
        "hk-client-sound-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("sound-test")
    ),
    ("hk-server-shell", true, 9, LATENCY, NEVER, None),
    ("hk-server-tempo", true, 8, TEMPO_POOL, NEVER, None),
    (
        "hk-server-surface-provider",
        true,
        0,
        LATENCY_HIGH,
        NEVER,
        None
    ),
    ("hk-server-display", true, 0, REALTIME, ON_FAILURE, None),
    (
        "hk-client-display-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("display-test")
    ),
    (
        "hk-client-terminal-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("terminal-test")
    ),
    ("hk-server-input", true, 2, REALTIME, NEVER, None),
    ("hk-server-usb", true, 2, LATENCY_HIGH, NEVER, None),
    (
        "hk-client-pointer-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("pointer-test")
    ),
    ("hk-server-holydoc", true, 7, LATENCY, NEVER, None),
    ("hk-server-workbench", true, 7, LATENCY, NEVER, None),
    ("hk-server-debug", true, 9, LATENCY, NEVER, None),
    ("hk-server-taskman", true, 7, LATENCY, NEVER, None),
    (
        "hk-client-dbg",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("dbg-test")
    ),
    (
        "hk-client-display-restart-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("display-restart-test")
    ),
    ("hk-server-checkpoint", true, 9, LATENCY, NEVER, None),
    (
        "hk-client-checkpoint-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("checkpoint")
    ),
    (
        "hk-server-memory-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("memory-test")
    ),
    ("hk-server-image", true, 9, THROUGHPUT, ON_FAILURE, None),
    (
        "hk-client-image-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("task-image")
    ),
    (
        "hk-client-swap-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("swap-test")
    ),
    (
        "hk-client-oom-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("oom-test")
    ),
    (
        "hk-client-oom-observer",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("oom-test")
    ),
    (
        "hk-client-numa-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("numa-test")
    ),
    (
        "hk-server-gpu-provider",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("gpu-alloc-test")
    ),
    (
        "hk-client-gpu-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("gpu-alloc-test")
    ),
    (
        "hk-server-tempo-test",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("tempo-test")
    ),
    (
        "hk-server-realtime",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("realtime-test")
    ),
    (
        "hk-server-wakeup",
        false,
        9,
        THROUGHPUT,
        NEVER,
        Some("wakeup-test")
    ),
];

/// 未在 [`HEAP_BYTES`] 登记的服务使用的默认堆大小。
pub const DEFAULT_HEAP_BYTES: usize = 512 * 1024;

/// 各服务首次分配时向内核申请的堆大小。
///
/// 服务入口通过 `hk_user_api::server_heap!()` 按包名查询，不再各自硬编码。
pub const HEAP_BYTES: &[(&str, usize)] = &[
    ("hk-server-ahci", 512 * 1024),
    ("hk-server-block", 512 * 1024),
    ("hk-server-checkpoint", 256 * 1024),
    ("hk-server-console", 512 * 1024),
    ("hk-server-debug", 512 * 1024),
    ("hk-server-display", 16 * 1024),
    ("hk-server-ext4", 2 * 1024 * 1024),
    ("hk-server-holydoc", 1024 * 1024),
    ("hk-server-holyfs", 2 * 1024 * 1024),
    ("hk-server-image", 2 * 1024 * 1024),
    ("hk-server-input", 512 * 1024),
    ("hk-server-memory", 1024 * 1024),
    ("hk-server-net", 768 * 1024),
    ("hk-server-nvme", 512 * 1024),
    ("hk-server-path", 2 * 1024 * 1024),
    ("hk-server-shell", 256 * 1024),
    ("hk-server-surface-provider", 64 * 1024),
    ("hk-server-taskman", 512 * 1024),
    ("hk-server-usb", 512 * 1024),
    ("hk-server-vfs", 2 * 1024 * 1024),
    ("hk-server-virtio", 512 * 1024),
    ("hk-server-virtio-net", 256 * 1024),
    ("hk-server-virtio-sound", 64 * 1024),
    ("hk-server-workbench", 1024 * 1024),
    ("hk-client-dbg", 64 * 1024),
    ("hk-client-swap-test", 2 * 1024 * 1024),
    ("hk-server-demo", 256 * 1024),
    ("hk-server-echo", 256 * 1024),
    ("hk-server-realtime", 64 * 1024),
    ("hk-server-tempo-test", 64 * 1024),
    ("hk-server-wakeup", 32 * 1024),
];

/// 查询服务堆大小；未登记的服务使用 [`DEFAULT_HEAP_BYTES`]。
pub fn heap_bytes(name: &str) -> usize {
    HEAP_BYTES
        .iter()
        .find(|(service, _)| *service == name)
        .map_or(DEFAULT_HEAP_BYTES, |(_, bytes)| *bytes)
}

#[cfg(test)]
mod heap_tests {
    use super::*;

    #[test]
    fn heap_sizes_are_registered_and_aligned() {
        for (name, bytes) in HEAP_BYTES {
            assert!(SERVERS.contains(name), "堆大小登记了未知服务：{name}");
            assert!(*bytes >= 4096, "{name} 的堆过小");
            assert_eq!(bytes % 4096, 0, "{name} 的堆未按页对齐");
        }
        assert_eq!(heap_bytes("hk-server-console"), 512 * 1024);
        assert_eq!(heap_bytes("unknown"), DEFAULT_HEAP_BYTES);
    }
}
