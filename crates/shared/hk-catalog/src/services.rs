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

/// 服务首次分配时向内核申请的堆大小。
///
/// 服务入口用 `hk_user_api::server_heap!(hk_catalog::HEAP_*)` 引用；
/// 常量在编译期内联，不把清单一表链进服务。
/// ahci 的堆大小。
pub const HEAP_AHCI: usize = 512 * 1024;
/// block 的堆大小。
pub const HEAP_BLOCK: usize = 512 * 1024;
/// checkpoint 的堆大小。
pub const HEAP_CHECKPOINT: usize = 256 * 1024;
/// console 的堆大小。
pub const HEAP_CONSOLE: usize = 512 * 1024;
/// debug 的堆大小。
pub const HEAP_DEBUG: usize = 512 * 1024;
/// display 的堆大小。
pub const HEAP_DISPLAY: usize = 16 * 1024;
/// ext4 的堆大小。
pub const HEAP_EXT4: usize = 2 * 1024 * 1024;
/// holydoc 的堆大小。
pub const HEAP_HOLYDOC: usize = 1024 * 1024;
/// holyfs 的堆大小。
pub const HEAP_HOLYFS: usize = 2 * 1024 * 1024;
/// image 的堆大小。
pub const HEAP_IMAGE: usize = 2 * 1024 * 1024;
/// input 的堆大小。
pub const HEAP_INPUT: usize = 512 * 1024;
/// memory 的堆大小。
pub const HEAP_MEMORY: usize = 1024 * 1024;
/// net 的堆大小。
pub const HEAP_NET: usize = 768 * 1024;
/// nvme 的堆大小。
pub const HEAP_NVME: usize = 512 * 1024;
/// path 的堆大小。
pub const HEAP_PATH: usize = 2 * 1024 * 1024;
/// shell 的堆大小。
pub const HEAP_SHELL: usize = 256 * 1024;
/// surface provider 的堆大小。
pub const HEAP_SURFACE_PROVIDER: usize = 64 * 1024;
/// taskman 的堆大小。
pub const HEAP_TASKMAN: usize = 512 * 1024;
/// usb 的堆大小。
pub const HEAP_USB: usize = 512 * 1024;
/// vfs 的堆大小。
pub const HEAP_VFS: usize = 2 * 1024 * 1024;
/// virtio net 的堆大小。
pub const HEAP_VIRTIO_NET: usize = 256 * 1024;
/// virtio sound 的堆大小。
pub const HEAP_VIRTIO_SOUND: usize = 64 * 1024;
/// virtio 的堆大小。
pub const HEAP_VIRTIO: usize = 512 * 1024;
/// workbench 的堆大小。
pub const HEAP_WORKBENCH: usize = 1024 * 1024;
/// dbg 的堆大小。
pub const HEAP_DBG: usize = 64 * 1024;
/// swap test 的堆大小。
pub const HEAP_SWAP_TEST: usize = 2 * 1024 * 1024;
/// demo 的堆大小。
pub const HEAP_DEMO: usize = 256 * 1024;
/// echo 的堆大小。
pub const HEAP_ECHO: usize = 256 * 1024;
/// realtime 的堆大小。
pub const HEAP_REALTIME: usize = 64 * 1024;
/// tempo 的堆大小。
pub const HEAP_TEMPO: usize = 64 * 1024;
/// tempo test 的堆大小。
pub const HEAP_TEMPO_TEST: usize = 64 * 1024;
/// wakeup 的堆大小。
pub const HEAP_WAKEUP: usize = 32 * 1024;

/// 服务名到堆大小的表；供校验与工具使用。
pub const HEAP_BYTES: &[(&str, usize)] = &[
    ("hk-server-ahci", HEAP_AHCI),
    ("hk-server-block", HEAP_BLOCK),
    ("hk-server-checkpoint", HEAP_CHECKPOINT),
    ("hk-server-console", HEAP_CONSOLE),
    ("hk-server-debug", HEAP_DEBUG),
    ("hk-server-display", HEAP_DISPLAY),
    ("hk-server-ext4", HEAP_EXT4),
    ("hk-server-holydoc", HEAP_HOLYDOC),
    ("hk-server-holyfs", HEAP_HOLYFS),
    ("hk-server-image", HEAP_IMAGE),
    ("hk-server-input", HEAP_INPUT),
    ("hk-server-memory", HEAP_MEMORY),
    ("hk-server-net", HEAP_NET),
    ("hk-server-nvme", HEAP_NVME),
    ("hk-server-path", HEAP_PATH),
    ("hk-server-shell", HEAP_SHELL),
    ("hk-server-surface-provider", HEAP_SURFACE_PROVIDER),
    ("hk-server-taskman", HEAP_TASKMAN),
    ("hk-server-usb", HEAP_USB),
    ("hk-server-vfs", HEAP_VFS),
    ("hk-server-virtio-net", HEAP_VIRTIO_NET),
    ("hk-server-virtio-sound", HEAP_VIRTIO_SOUND),
    ("hk-server-virtio", HEAP_VIRTIO),
    ("hk-server-workbench", HEAP_WORKBENCH),
    ("hk-client-dbg", HEAP_DBG),
    ("hk-client-swap-test", HEAP_SWAP_TEST),
    ("hk-server-demo", HEAP_DEMO),
    ("hk-server-echo", HEAP_ECHO),
    ("hk-server-realtime", HEAP_REALTIME),
    ("hk-server-tempo", HEAP_TEMPO),
    ("hk-server-tempo-test", HEAP_TEMPO_TEST),
    ("hk-server-wakeup", HEAP_WAKEUP),
];

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
        assert_eq!(HEAP_CONSOLE, 512 * 1024);
    }
}
