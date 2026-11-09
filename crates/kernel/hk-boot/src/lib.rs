//! 引导适配：把不同 bootloader/固件协议统一转换为平台无关的 [`BootInfo`]。
//!
//! 引导后端通过 [`BootBackend`] 提供启动信息与命令行：x86_64 使用 Limine，
//! 直接 `-kernel` 启动的 AArch64/RISC-V 使用 DTB 后端。

#![no_std]

extern crate alloc;

use core::fmt;

use hk_base::{BootInfo, BootInfoError, PhysAddr};

/// 设备树（DTB）引导后端。
pub mod dtb;
/// Limine 引导后端。
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
pub mod limine;

/// 引导后端：产出平台无关的启动信息。
///
/// 平台 crate 直接选择后端：[`limine::Limine`] 覆盖 x86_64/AArch64/RISC-V，
/// 直接 `-kernel` 启动可用 [`dtb::DtbBoot`]。crate 根不导出任何架构专属 API。
pub trait BootBackend {
    /// 构造启动信息。
    fn boot_info(&self) -> Result<BootInfo, BootError>;

    /// 返回内核命令行；后端不提供时为 `None`。
    fn cmdline(&self) -> Option<&'static str> {
        None
    }
}

/// 引导适配错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootError {
    /// 引导后端没有回应某类请求。
    MissingResponse(&'static str),
    /// 引导后端不支持请求的协议版本。
    UnsupportedBaseRevision {
        /// 实际生效的版本。
        actual: u64,
    },
    /// 地址无法从虚拟地址换算为物理地址。
    AddressTranslation {
        /// 出错的虚拟地址。
        address: u64,
    },
    /// 模块路径不是合法 UTF-8。
    InvalidModuleName,
    /// 内存 map 中没有描述内核镜像所在区域。
    MissingKernelRegion {
        /// 内核镜像起始物理地址。
        base: PhysAddr,
    },
    /// 启动信息校验失败。
    InvalidBootInfo(BootInfoError),
}

impl fmt::Display for BootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingResponse(name) => write!(formatter, "引导后端未回应 {name} 请求"),
            Self::UnsupportedBaseRevision { actual } => {
                write!(formatter, "引导协议版本不受支持：{actual}")
            }
            Self::AddressTranslation { address } => {
                write!(formatter, "地址 {address:#x} 无法换算为物理地址")
            }
            Self::InvalidModuleName => formatter.write_str("模块路径不是合法 UTF-8"),
            Self::MissingKernelRegion { base } => {
                write!(formatter, "内存 map 未描述内核镜像区域：{base:?}")
            }
            Self::InvalidBootInfo(error) => write!(formatter, "启动信息校验失败：{error}"),
        }
    }
}

impl core::error::Error for BootError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::InvalidBootInfo(error) => Some(error),
            _ => None,
        }
    }
}
