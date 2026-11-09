//! 架构原语：包装外部 `x86_64` 与 `aarch64-cpu` crate。
//!
//! 外部库类型只在本 crate 内部出现；对平台只暴露与地址无关的寄存器、指令与
//! 屏障封装。各架构的模块按 `target_arch` 条件编译。

#![no_std]

#[cfg(target_arch = "aarch64")]
mod aarch64_impl;
#[cfg(target_arch = "x86_64")]
mod x86_64_impl;

#[cfg(target_arch = "aarch64")]
pub use aarch64_impl::*;
#[cfg(target_arch = "x86_64")]
pub use x86_64_impl::*;
