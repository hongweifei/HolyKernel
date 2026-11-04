//! 通用 SMP 状态机。
//!
//! 不依赖架构：TLB 刷新与 IPI 发送由平台以闭包提供。

#![no_std]

extern crate alloc;

pub mod ap_pool;
pub mod idle;
pub mod tlb;

pub use ap_pool::{AP_POOL, ApPool};
pub use idle::IdleState;
pub use tlb::{TlbHardware, TlbShootdown};
