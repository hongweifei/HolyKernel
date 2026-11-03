//! 架构相关的系统调用入口。

/// 系统调用返回的寄存器映射。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyscallResult {
    /// 返回值。
    pub value: u64,
    /// 消息字或附加返回值。
    pub word: u64,
    /// 发送者端点或附加返回值。
    pub sender: u64,
    /// 随消息接收的句柄编号；`0` 表示没有句柄。
    pub extra: u64,
}

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::call;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::call;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("当前架构未提供系统调用入口");
