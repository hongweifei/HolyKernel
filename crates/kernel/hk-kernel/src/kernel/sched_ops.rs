//! Task 生命周期与 Beat 调度决策。
//!
//! Task 创建、契约计算与退出回收在 `cadence`；入队、派发、阻塞唤醒与预算补充
//! 在 `switch`；借贷契约选择的单元测试在 `tests`。

mod cadence;
mod switch;

#[cfg(test)]
mod tests;
