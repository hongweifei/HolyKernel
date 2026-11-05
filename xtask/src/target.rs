//! 构建目标。

use clap::ValueEnum;

/// 命令行指定的目标集合。
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Selection {
    /// Hosted 模拟器
    #[value(name = "hosted")]
    Hosted,
    /// x86_64 QEMU
    #[value(name = "x86_64-qemu")]
    X86_64Qemu,
    /// AArch64 QEMU
    #[value(name = "aarch64-qemu")]
    Aarch64Qemu,
    /// 全部目标
    #[value(name = "all")]
    All,
}

static HOSTED_TARGETS: [Target; 1] = [Target::Hosted];
static X86_64_TARGETS: [Target; 1] = [Target::X86_64Qemu];
static AARCH64_TARGETS: [Target; 1] = [Target::Aarch64Qemu];
static ALL_TARGETS: [Target; 3] = [Target::Hosted, Target::X86_64Qemu, Target::Aarch64Qemu];

impl Selection {
    /// 展开为具体目标。
    pub fn targets(self) -> &'static [Target] {
        match self {
            Self::Hosted => &HOSTED_TARGETS,
            Self::X86_64Qemu => &X86_64_TARGETS,
            Self::Aarch64Qemu => &AARCH64_TARGETS,
            Self::All => &ALL_TARGETS,
        }
    }
}

static HOSTED_PACKAGES: [&str; 1] = ["hk-hosted"];
static KERNEL_PACKAGES: [&str; 7] = [
    "hk-hal",
    "hk-handle",
    "hk-ipc",
    "hk-mem",
    "hk-kernel",
    "hk-root-server",
    "hk-test",
];
static X86_64_PACKAGES: [&str; 13] = [
    "hk-hal",
    "hk-handle",
    "hk-ipc",
    "hk-mem",
    "hk-kernel",
    "hk-root-server",
    "hk-server-api",
    "hk-server-demo",
    "hk-server-block",
    "hk-server-netstat",
    "hk-server-netsock",
    "hk-test",
    "hk-x86_64",
];

/// 单个构建目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Hosted 模拟器。
    Hosted,
    /// x86_64 QEMU。
    X86_64Qemu,
    /// AArch64 QEMU。
    Aarch64Qemu,
}

impl Target {
    /// 返回目标三元组；Hosted 使用宿主三元组。
    pub const fn triple(self) -> Option<&'static str> {
        match self {
            Self::Hosted => None,
            Self::X86_64Qemu => Some("x86_64-unknown-none"),
            Self::Aarch64Qemu => Some("aarch64-unknown-none"),
        }
    }

    /// 返回该目标构建的包集合。
    pub const fn packages(self) -> &'static [&'static str] {
        match self {
            Self::Hosted => &HOSTED_PACKAGES,
            Self::X86_64Qemu => &X86_64_PACKAGES,
            Self::Aarch64Qemu => &KERNEL_PACKAGES,
        }
    }

    /// 判断是否为 Hosted 目标。
    pub const fn is_hosted(self) -> bool {
        matches!(self, Self::Hosted)
    }
}
