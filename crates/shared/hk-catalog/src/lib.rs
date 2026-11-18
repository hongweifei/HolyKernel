//! 服务目录：镜像构建、启动白名单、桌面排序、重启策略、测试模式与默认 Cadence
//! 的单一清单。
//!
//! 新增服务时只修改 [`SERVICES`]；构建工具、Root Server 与平台装载从同一
//! 清单读取名称、启动标记、桌面排序、重启策略、opt-in 标记与默认调度契约。

#![no_std]

use core::iter::Iterator;

use hk_cadence::{CadenceClass, CadenceSpec, Lending, millis};

/// 服务退出后的重启策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartPolicy {
    /// 不自动重启。
    Never,
    /// 非零退出码时重启。
    OnFailure,
    /// 退出即重启。
    Always,
}

const NEVER: RestartPolicy = RestartPolicy::Never;
const ON_FAILURE: RestartPolicy = RestartPolicy::OnFailure;
const ALWAYS: RestartPolicy = RestartPolicy::Always;

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
    /// 退出后的重启策略。
    pub restart: RestartPolicy,
    /// 非默认启动服务的 cmdline 标记；出现时随镜像装载。
    pub opt_in: Option<&'static str>,
}

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

/// 按名称查找服务。
pub fn service(name: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|service| service.name == name)
}

/// 判断服务是否在默认启动白名单中。
pub fn boot_service(name: &str) -> bool {
    service(name).is_some_and(|service| service.boot)
}
/// 逐服务接入懒 TaskImage 的名单；未列出的服务仍由装载器常驻映射。
pub const LAZY_TASK_IMAGES: &[&str] = &[
    "hk-client-checkpoint-test",
    "hk-client-console-test",
    "hk-client-dbg",
    "hk-client-display-restart-test",
    "hk-client-display-test",
    "hk-client-gpu-test",
    "hk-client-image-test",
    "hk-client-netstat",
    "hk-client-netsock",
    "hk-client-numa-test",
    "hk-client-oom-observer",
    "hk-client-oom-test",
    "hk-client-pointer-test",
    "hk-client-sound-test",
    "hk-client-swap-test",
    "hk-client-terminal-test",
    "hk-server-ahci",
    "hk-server-block",
    "hk-server-checkpoint",
    "hk-server-console",
    "hk-server-debug",
    "hk-server-demo",
    "hk-server-device",
    "hk-server-display",
    "hk-server-echo",
    "hk-server-ext4",
    "hk-server-fault",
    "hk-server-fork",
    "hk-server-gpu-provider",
    "hk-server-holydoc",
    "hk-server-holyfs",
    "hk-server-input",
    "hk-server-latency",
    "hk-server-memory",
    "hk-server-memory-test",
    "hk-server-net",
    "hk-server-nvme",
    "hk-server-path",
    "hk-server-realtime",
    "hk-server-shell",
    "hk-server-spin",
    "hk-server-surface-provider",
    "hk-server-taskman",
    "hk-server-tempo",
    "hk-server-tempo-test",
    "hk-server-usb",
    "hk-server-vfs",
    "hk-server-virtio",
    "hk-server-virtio-net",
    "hk-server-virtio-sound",
    "hk-server-wakeup",
    "hk-server-workbench",
];

/// 必须在启动阶段常驻映射的服务。
///
/// 驱动的中断路径不能等待用户态 Provider，否则缺页会以 `PROVIDER_FAULT_EXIT`
/// 终止任务；这些服务即使出现在懒装载名单里也强制 eager。
pub const RESIDENT_SERVICES: &[&str] = &["hk-server-virtio"];

/// 判断服务是否强制常驻映射。
pub fn resident_service(name: &str) -> bool {
    RESIDENT_SERVICES.contains(&name)
}

/// 判断服务是否走懒 TaskImage 装载。
pub fn lazy_task_image(name: &str) -> bool {
    !resident_service(name) && LAZY_TASK_IMAGES.contains(&name)
}
/// 使用用户态 Image Server 的隔离服务名单。
pub const USER_IMAGE_SERVICES: &[&str] = &[
    "hk-client-image-test",
    "hk-server-echo",
    "hk-server-demo",
    "hk-server-ext4",
    "hk-server-vfs",
    "hk-server-path",
    "hk-server-memory",
    "hk-server-device",
    "hk-server-taskman",
    "hk-server-holyfs",
    "hk-server-block",
    "hk-server-shell",
    "hk-server-tempo",
    "hk-server-holydoc",
    "hk-server-workbench",
    "hk-server-debug",
    "hk-server-checkpoint",
    "hk-server-display",
    "hk-server-input",
];

/// 判断服务是否在用户态 Image Server 模式下提供页。
pub fn user_image_service(name: &str) -> bool {
    USER_IMAGE_SERVICES.contains(&name)
}

/// 返回服务启动阶段；数值越小越早。
///
/// 阶段只表达依赖方向，不替代同一阶段内的 catalog 顺序。Image Server 必须
/// 先于所有用户态 Provider 客户端；设备管理先于驱动；块服务先于文件系统。
pub fn boot_phase(name: &str) -> u8 {
    match name {
        "hk-server-image" => 0,
        "hk-server-memory" | "hk-server-device" => 1,
        "hk-server-virtio"
        | "hk-server-virtio-sound"
        | "hk-server-virtio-net"
        | "hk-server-ahci"
        | "hk-server-nvme"
        | "hk-server-usb"
        | "hk-server-net" => 2,
        "hk-server-block" => 3,
        "hk-server-ext4" | "hk-server-holyfs" => 4,
        "hk-server-vfs" => 5,
        "hk-server-path" => 6,
        "hk-server-console"
        | "hk-server-display"
        | "hk-server-input"
        | "hk-server-taskman"
        | "hk-server-shell"
        | "hk-server-tempo"
        | "hk-server-holydoc"
        | "hk-server-workbench"
        | "hk-server-debug"
        | "hk-server-checkpoint" => 7,
        _ if name.starts_with("hk-client-") => 9,
        _ => 8,
    }
}

/// 返回桌面模式排序键；未登记服务排在最后。
pub fn desktop_order(name: &str) -> u8 {
    service(name).map_or(9, |service| service.desktop_order)
}

/// 返回服务的默认 Cadence；未登记服务取 [`DEFAULT_CADENCE`]。
pub fn default_cadence(name: &str) -> CadenceSpec {
    service(name).map_or(DEFAULT_CADENCE, |service| service.cadence)
}

/// 返回服务的重启策略；未登记服务不自动重启。
pub fn restart_policy(name: &str) -> RestartPolicy {
    service(name).map_or(RestartPolicy::Never, |service| service.restart)
}

/// 判断服务默认是否属于 Realtime 类别。
pub fn is_realtime(name: &str) -> bool {
    default_cadence(name).class == CadenceClass::Realtime
}

/// 判断服务是否持有调度策略授权；当前只有 Tempo。
pub fn policy_authority(name: &str) -> bool {
    name == "hk-server-tempo"
}

/// OOM 选择时豁免的服务：内存、调试、检查点、任务管理器与观察端。
pub fn oom_exempt(name: &str) -> bool {
    name == "root"
        || name.starts_with("hk-server-memory")
        || name == "hk-client-oom-observer"
        || name.starts_with("hk-server-debug")
        || name.starts_with("hk-server-checkpoint")
        || name.starts_with("hk-server-taskman")
}

/// 需要独立 Gpu Capacity 的服务。
pub const GPU_CAPACITY_SERVICES: &[&str] = &["hk-server-gpu-provider", "hk-client-gpu-test"];

/// 每个 Gpu 服务取得的 Capacity 字节数。
pub const GPU_CAPACITY_BYTES: usize = 1024 * 1024;

/// 判断服务能否在自检标记下修改 OOM 策略。
pub fn oom_policy_authority(name: &str, cmdline: &str) -> bool {
    name == "hk-client-oom-test" && cmdline.contains("oom-test")
}

/// 内核态 shell 的 `net` 命令查询的状态客户端。
pub const NETSTAT_CLIENT: &str = "hk-client-netstat";

/// 启动模式：cmdline 命中全部 token 时，只装载 `services` 中的镜像。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootMode {
    /// 需要同时出现在 boot cmdline 中的标记。
    pub tokens: &'static [&'static str],
    /// 该模式装载的服务集合。
    pub services: &'static [&'static str],
}

/// 测试与桌面模式；顺序即匹配优先级，首个命中生效。
pub const BOOT_MODES: &[BootMode] = &[
    BootMode {
        tokens: &["tempo-test"],
        services: &["hk-server-tempo", "hk-server-tempo-test"],
    },
    BootMode {
        tokens: &["wakeup-test"],
        services: &["hk-server-wakeup"],
    },
    BootMode {
        tokens: &["dbg-test"],
        services: &["hk-client-dbg"],
    },
    BootMode {
        tokens: &["rng-test"],
        services: &["hk-server-device", "hk-server-virtio"],
    },
    BootMode {
        tokens: &["vcon-test"],
        services: &["hk-server-device", "hk-server-virtio"],
    },
    BootMode {
        tokens: &["sound-test"],
        services: &[
            "hk-server-device",
            "hk-server-virtio-sound",
            "hk-client-sound-test",
        ],
    },
    BootMode {
        tokens: &["gpu-test"],
        services: &["hk-server-device", "hk-server-virtio"],
    },
    BootMode {
        tokens: &["console-test"],
        services: &["hk-server-console", "hk-client-console-test"],
    },
    BootMode {
        tokens: &["scanout-test"],
        services: &[
            "hk-server-display",
            "hk-server-surface-provider",
            "hk-server-memory",
            "hk-server-device",
            "hk-server-virtio",
        ],
    },
    BootMode {
        tokens: &["display-test", "gpu.scanout"],
        services: &[
            "hk-server-display",
            "hk-client-display-test",
            "hk-server-surface-provider",
            "hk-server-memory",
            "hk-server-device",
            "hk-server-virtio",
        ],
    },
    BootMode {
        tokens: &["display-test"],
        services: &[
            "hk-server-display",
            "hk-client-display-test",
            "hk-server-surface-provider",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["holydoc-resize-test"],
        services: &[
            "hk-server-display",
            "hk-server-holydoc",
            "hk-server-holyfs",
            "hk-server-vfs",
            "hk-server-virtio",
            "hk-server-block",
            "hk-server-device",
            "hk-server-input",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["holydoc-test"],
        services: &[
            "hk-server-display",
            "hk-server-holydoc",
            "hk-server-holyfs",
            "hk-server-vfs",
            "hk-server-virtio",
            "hk-server-block",
            "hk-server-device",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["workbench-test"],
        services: &[
            "hk-server-display",
            "hk-server-workbench",
            "hk-server-holyfs",
            "hk-server-vfs",
            "hk-server-virtio",
            "hk-server-block",
            "hk-server-device",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["debug-test"],
        services: &["hk-server-display", "hk-server-debug", "hk-server-memory"],
    },
    BootMode {
        tokens: &["debug-crash"],
        services: &["hk-server-display", "hk-server-debug", "hk-server-memory"],
    },
    BootMode {
        tokens: &["taskman-test"],
        services: &["hk-server-display", "hk-server-taskman", "hk-server-memory"],
    },
    BootMode {
        tokens: &["display-restart-test"],
        services: &[
            "hk-server-display",
            "hk-server-console",
            "hk-server-taskman",
            "hk-server-debug",
            "hk-server-workbench",
            "hk-server-holydoc",
            "hk-client-display-restart-test",
            "hk-server-memory",
            "hk-server-device",
            "hk-server-virtio",
            "hk-server-block",
            "hk-server-holyfs",
            "hk-server-vfs",
        ],
    },
    BootMode {
        tokens: &["hotswap-test"],
        services: &["hk-server-echo", "hk-server-spin", "hk-server-fault"],
    },
    BootMode {
        tokens: &["checkpoint-test"],
        services: &["hk-server-memory"],
    },
    BootMode {
        tokens: &["memory-test"],
        services: &["hk-server-memory-test"],
    },
    BootMode {
        tokens: &["task-image-user"],
        services: &["hk-server-image", "hk-client-image-test", "hk-server-echo"],
    },
    BootMode {
        tokens: &["task-image-restart"],
        services: &["hk-server-image", "hk-client-image-test"],
    },
    BootMode {
        tokens: &["task-image"],
        services: &["hk-client-image-test"],
    },
    BootMode {
        tokens: &["swap-test"],
        services: &["hk-server-memory", "hk-client-swap-test"],
    },
    BootMode {
        tokens: &["oom-test"],
        services: &["hk-client-oom-test", "hk-client-oom-observer"],
    },
    BootMode {
        tokens: &["gpu-alloc-test"],
        services: &["hk-server-gpu-provider", "hk-client-gpu-test"],
    },
    BootMode {
        tokens: &["numa-test"],
        services: &["hk-server-memory", "hk-client-numa-test"],
    },
    BootMode {
        tokens: &["checkpoint-srv-test"],
        services: &[
            "hk-server-echo",
            "hk-server-path",
            "hk-server-checkpoint",
            "hk-client-checkpoint-test",
            "hk-server-device",
            "hk-server-virtio",
            "hk-server-block",
            "hk-server-holyfs",
            "hk-server-vfs",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["desktop"],
        services: &[
            "hk-server-display",
            "hk-server-console",
            "hk-server-device",
            "hk-server-input",
            "hk-server-usb",
            "hk-server-virtio",
            "hk-server-holyfs",
            "hk-server-vfs",
            "hk-server-path",
            "hk-server-holydoc",
            "hk-server-workbench",
            "hk-server-taskman",
            "hk-server-shell",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["pointer-test"],
        services: &[
            "hk-server-display",
            "hk-server-device",
            "hk-server-input",
            "hk-client-pointer-test",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["input-test", "terminal-test"],
        services: &[
            "hk-server-display",
            "hk-server-device",
            "hk-server-console",
            "hk-client-terminal-test",
            "hk-server-input",
            "hk-server-memory",
        ],
    },
    BootMode {
        tokens: &["terminal-test"],
        services: &[
            "hk-server-display",
            "hk-server-console",
            "hk-client-terminal-test",
            "hk-server-memory",
        ],
    },
];

/// 默认启动时按需装载、不常驻的服务。
///
/// `netstat` 只由 `net` 命令启动；其余由桌面或测试模式装载。
pub const DEFAULT_HIDDEN: &[&str] = &[
    "hk-client-netstat",
    "hk-server-holydoc",
    "hk-server-workbench",
    "hk-server-debug",
    "hk-server-taskman",
    "hk-server-shell",
    "hk-client-display-restart-test",
    "hk-server-checkpoint",
    "hk-client-checkpoint-test",
    "hk-server-console",
    "hk-client-console-test",
    "hk-server-display",
    "hk-client-display-test",
    "hk-client-terminal-test",
    "hk-server-input",
    "hk-client-pointer-test",
];

/// 仅在网络测试模式下装载的服务。
pub const NETWORK_SERVICES: &[&str] =
    &["hk-server-virtio-net", "hk-server-net", "hk-client-netsock"];

/// 返回 cmdline 命中的启动模式；首个 token 全部出现的条目生效。
pub fn boot_mode(cmdline: &str) -> Option<&'static BootMode> {
    BOOT_MODES
        .iter()
        .find(|mode| mode.tokens.iter().all(|token| cmdline.contains(token)))
}

/// 判断服务是否随本次启动装载：默认白名单，或 opt-in 标记出现。
pub fn activated(service: &Service, cmdline: &str) -> bool {
    service.boot || service.opt_in.is_some_and(|token| cmdline.contains(token))
}

/// 判断 cmdline 是否启用网络服务。
pub fn network_enabled(cmdline: &str) -> bool {
    cmdline.contains("net-test")
}

/// 默认模式是否装载该服务。
pub fn default_visible(name: &str, network: bool) -> bool {
    !DEFAULT_HIDDEN.contains(&name) && (network || !NETWORK_SERVICES.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_task_images_exist() {
        for name in LAZY_TASK_IMAGES {
            assert!(service(name).is_some(), "懒镜像未登记：{name}");
        }
    }

    #[test]
    fn boot_phases_follow_service_dependencies() {
        assert!(boot_phase("hk-server-image") < boot_phase("hk-server-device"));
        assert!(boot_phase("hk-server-device") < boot_phase("hk-server-virtio"));
        assert!(boot_phase("hk-server-virtio") < boot_phase("hk-server-block"));
        assert!(boot_phase("hk-server-block") < boot_phase("hk-server-ext4"));
        assert!(boot_phase("hk-server-ext4") < boot_phase("hk-server-vfs"));
        assert!(boot_phase("hk-server-vfs") < boot_phase("hk-server-path"));
        assert!(boot_phase("hk-server-path") < boot_phase("hk-client-image-test"));
        assert!(boot_phase("hk-server-memory") < boot_phase("hk-server-display"));
    }

    #[test]
    fn resident_services_override_lazy_loading() {
        for name in RESIDENT_SERVICES {
            assert!(service(name).is_some(), "常驻服务未登记：{name}");
            assert!(!lazy_task_image(name), "常驻服务不能懒装载：{name}");
        }
        assert!(resident_service("hk-server-virtio"));
        assert!(!resident_service("hk-server-echo"));
        assert!(lazy_task_image("hk-server-echo"));
    }

    #[test]
    fn realtime_catalog_load_fits_period() {
        let mut total = 0_u64;
        for service in SERVICES {
            if service.boot && service.name != "hk-server-tempo" && is_realtime(service.name) {
                total = total.saturating_add(service.cadence.budget.ticks());
            }
        }
        total = total.saturating_add(default_cadence("hk-server-tempo").budget.ticks());
        // 预留给动态准入的最小 1 ms Realtime 子契约。
        assert!(
            total.saturating_add(millis(1).ticks()) <= millis(16).ticks(),
            "启动清单的 Realtime 预算必须为动态准入留出余量"
        );
    }

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
        assert_eq!(restart_policy("hk-server-unknown"), RestartPolicy::Never);
    }

    #[test]
    fn modes_only_reference_registered_services() {
        for mode in BOOT_MODES {
            assert!(!mode.tokens.is_empty());
            assert!(!mode.services.is_empty());
            for name in mode.services {
                assert!(service(name).is_some(), "模式引用了未登记服务：{name}");
            }
        }
    }

    #[test]
    fn default_lists_only_reference_registered_services() {
        for name in DEFAULT_HIDDEN.iter().chain(NETWORK_SERVICES) {
            assert!(service(name).is_some(), "默认列表引用了未登记服务：{name}");
        }
    }

    #[test]
    fn opt_in_services_are_not_booted_by_default() {
        for service in SERVICES {
            if let Some(token) = service.opt_in {
                assert!(!service.boot, "{} 已默认启动，不需要 opt-in", service.name);
                assert!(!token.is_empty());
            }
        }
    }

    #[test]
    fn mode_tokens_select_their_services() {
        for mode in BOOT_MODES {
            let cmdline = mode.tokens.join(" ");
            let selected = boot_mode(&cmdline).expect("cmdline 应命中模式");
            assert_eq!(
                selected.services, mode.services,
                "cmdline {cmdline} 命中了错误的模式"
            );
        }
    }

    #[test]
    fn mode_services_with_opt_in_are_activated() {
        for mode in BOOT_MODES {
            let cmdline = mode.tokens.join(" ");
            for name in mode.services {
                let entry = service(name).expect("模式服务必须已登记");
                if entry.opt_in.is_some() {
                    assert!(
                        activated(entry, &cmdline),
                        "模式 {mode:?} 的 cmdline 无法装载 {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn policy_lists_reference_registered_services() {
        for name in GPU_CAPACITY_SERVICES {
            assert!(service(name).is_some(), "GPU 容量服务未登记：{name}");
        }
        assert!(service(NETSTAT_CLIENT).is_some());
        assert!(service("hk-client-oom-test").is_some());
        assert!(oom_exempt("root"));
        assert!(oom_exempt("hk-server-memory"));
        assert!(oom_exempt("hk-client-oom-observer"));
        assert!(!oom_exempt("hk-client-console-test"));
        assert!(oom_policy_authority("hk-client-oom-test", "oom-test"));
        assert!(!oom_policy_authority("hk-client-oom-test", "swap-test"));
    }

    #[test]
    fn restart_policies_match_service_contracts() {
        assert_eq!(restart_policy("hk-server-device"), RestartPolicy::Always);
        assert_eq!(
            restart_policy("hk-server-virtio-net"),
            RestartPolicy::Always
        );
        assert_eq!(
            restart_policy("hk-server-display"),
            RestartPolicy::OnFailure
        );
        assert_eq!(restart_policy("hk-server-ext4"), RestartPolicy::OnFailure);
        assert_eq!(restart_policy("hk-server-memory"), RestartPolicy::Never);
    }
}
