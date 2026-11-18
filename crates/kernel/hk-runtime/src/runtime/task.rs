//! 任务生命周期与调度。
//!
//! 实现按职责拆分：调度、退出与阻塞唤醒在 `lifecycle`，fork 在 `fork`，
//! 检查点与装载在 `checkpoint` 与 `spawn`。各块均为 `UserRuntime` 的固有方法，
//! 调用点无需改动。

mod checkpoint;
mod fork;
mod lifecycle;
mod spawn;
