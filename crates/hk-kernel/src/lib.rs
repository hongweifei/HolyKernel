//! 微内核。
//!
//! 内核提供 TCB、VSpace、HandleTable、Endpoint、Notification、Frame 与 Untyped
//! 机制；调度、IPC 分派与对象回收在 [`Kernel`] 中组合，策略位于用户态服务器。
//!
//! Hosted 目标把线程体交给仿真器执行（`hk-sim`）；内核只维护可运行队列、阻塞
//! 原因与 IPC 状态。

#![no_std]

extern crate alloc;

pub mod allocator;
pub mod error;
pub mod kernel;
pub mod object;
pub mod sched;
pub mod tcb;
pub mod user;

pub use allocator::BumpHeap;
pub use error::KernelError;
pub use kernel::{Kernel, Received, TRACE_CAPACITY};
pub use object::{KernelObject, ObjectPayload};
pub use sched::Scheduler;
pub use tcb::{
    BlockReason, PendingCall, PendingDelivery, Priority, ReplyToken, Tcb, TcbId, ThreadState,
};

#[cfg(test)]
mod tests;
