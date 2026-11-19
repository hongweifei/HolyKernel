//! Task 生命周期与 Beat 调度决策。
//!
//! Task 创建与契约计算在 `task`；入队、派发、阻塞唤醒与预算补充在 `switch`；
//! 契约变更后的 placement 刷新在 `refresh`；退出、端点回收与子契约提升在
//! `lifecycle`；借贷契约选择的单元测试在 `tests`。

mod lifecycle;
mod refresh;
mod switch;
mod task;

#[cfg(test)]
mod tests;
