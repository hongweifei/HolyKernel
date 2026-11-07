//! 服务二进制与镜像模块清单（单一事实来源）。

/// 随镜像构建和打包的服务；顺序即启动装载顺序。
pub const SERVERS: &[&str] = &[
    "hk-server-demo",
    "hk-server-echo",
    "hk-server-device",
    "hk-server-spin",
    "hk-server-virtio",
    "hk-server-memory",
    "hk-server-holyfs",
    "hk-server-vfs",
    "hk-server-path",
    "hk-server-fault",
    "hk-server-fork",
    "hk-server-latency",
    "hk-server-block",
    "hk-server-virtio-net",
    "hk-server-net",
    "hk-server-netstat",
    "hk-server-netsock",
    "hk-server-console",
    "hk-server-console-test",
    "hk-server-shell",
    "hk-server-display",
    "hk-server-display-test",
    "hk-server-terminal-test",
    "hk-server-input",
    "hk-server-pointer-test",
    "hk-server-holydoc",
    "hk-server-workbench",
    "hk-server-debug",
    "hk-server-taskman",
    "hk-server-dbg",
    "hk-server-display-restart-test",
    "hk-server-checkpoint",
    "hk-server-checkpoint-test",
];

/// 随镜像打包的 HolyShell 脚本。
pub const SCRIPT_MODULES: &[&str] = &["boot.hsh", "services.hsh"];
