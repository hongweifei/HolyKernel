//! 架构标识与 CPU 标识。

use core::fmt;

/// 目标架构。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    /// x86_64。
    X86_64,
    /// AArch64。
    Aarch64,
    /// RISC-V 64。
    Riscv64,
    /// Hosted 模拟器。
    Hosted,
}

impl Arch {
    /// 返回架构名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Riscv64 => "riscv64",
            Self::Hosted => "hosted",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// 逻辑 CPU 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CpuId(u32);

impl CpuId {
    /// 构造 CPU 标识。
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// 返回 CPU 序号。
    pub const fn get(self) -> u32 {
        self.0
    }
}
