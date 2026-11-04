//! 通用 SMP 状态机。
//!
//! 不依赖架构：TLB 刷新与 IPI 发送由平台以闭包提供。

#![no_std]

pub mod idle;
pub mod tlb;

pub use idle::IdleState;
pub use tlb::{TlbHardware, TlbShootdown};
