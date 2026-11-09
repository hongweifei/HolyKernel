//! Tempo 策略服务协议。
//!
//! 调用方把目标 Cadence 的 80 字节快照放在消息载荷里发给 Tempo；Tempo 在自身
//! 契约范围内派生子契约，并把新契约句柄随回复转移给调用方。调用方随后用
//! `SYS_TASK_ATTACH_CADENCE(task = 0, cadence)` 把它附加到自己。

/// 准入请求：载荷为 `CadenceSnapshot` 编码。
pub const TEMPO_REQUEST: u64 = crate::labels::TEMPO;
/// 准入成功：`word` 为新契约 epoch，随消息转移一个 Cadence 句柄。
pub const TEMPO_RESPONSE: u64 = crate::labels::TEMPO + 1;
/// 准入拒绝：`word` 为原因码。
pub const TEMPO_REJECTED: u64 = crate::labels::TEMPO + 2;

/// 拒绝原因：请求载荷无效。
pub const REJECT_INVALID: u64 = 1;
/// 拒绝原因：超出 Tempo 契约范围。
pub const REJECT_OUT_OF_RANGE: u64 = 2;
