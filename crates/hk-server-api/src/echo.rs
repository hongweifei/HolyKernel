//! echo 服务的请求标签。
//!
//! 普通请求以 `label + 1` 回复；计算请求由工作线程处理后用独立标签回复，
//! 因此多个请求可以并行完成、乱序返回。

/// 计算请求：`word` 为任务编号。
pub const WORK_REQUEST: u64 = 0xE00;
/// 计算回复：`word` 为结果。
pub const WORK_RESPONSE: u64 = 0xE01;
/// 并发统计请求。
pub const WORK_STATS: u64 = 0xE02;
/// 并发统计回复：`word` 为观测到的最大同时运行任务数。
pub const WORK_STATS_RESPONSE: u64 = 0xE03;
/// 工作线程完成通知：`word` 为任务 id，由服务自己发给自己的端点。
pub const WORK_DONE: u64 = 0xE04;
/// 每个计算任务的忙等轮数。
pub const WORK_ROUNDS: u64 = 2_000_000;
