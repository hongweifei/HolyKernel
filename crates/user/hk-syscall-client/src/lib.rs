//! 零分配的系统调用薄客户端。
//!
//! 只做参数整理与返回值解包，供并发库与不需要服务运行时的构件使用；
//! HMP、设备、帧缓冲与运行时样板等服务面留在 `hk-server-api`。

#![no_std]

mod arch;
pub mod root;
pub mod syscall;

pub use arch::call;
pub use syscall::*;

pub(crate) use arch::call as syscall_call;
