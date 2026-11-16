//! 全局内存压力与 OOM 策略。
//!
//! 压力按 FrameBank 全局空闲比例计算；低于阈值时先回收当前任务的可回收页，
//! 仍不足才按 Ledger 用量选择 victim。Root 可配置阈值与动作，所有决定写入
//! 最近一次审计记录。

use alloc::string::String;

/// OOM 阈值与动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OomPolicy {
    /// 空闲内存低于该百分比时触发回收/OOM。
    pub min_free_percent: u8,
    /// 0 只观察并回收；1 允许终止 victim。
    pub action: u8,
}

impl Default for OomPolicy {
    fn default() -> Self {
        Self {
            min_free_percent: 10,
            action: 1,
        }
    }
}

/// 最近一次 OOM 审计记录。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OomEvent {
    /// 触发的任务名。
    pub victim: String,
    /// 触发时的全局空闲百分比。
    pub free_percent: u64,
    /// 本轮回收字节数。
    pub reclaimed: u64,
    /// 是否执行了终止。
    pub killed: bool,
}

/// OOM 状态。
#[derive(Clone, Debug, Default)]
pub struct OomState {
    /// 当前策略。
    pub policy: OomPolicy,
    /// 最近一次记录。
    pub last: Option<OomEvent>,
    /// 累计触发次数。
    pub events: u64,
    /// 累计终止次数。
    pub kills: u64,
    /// 累计回收字节数。
    pub reclaimed: u64,
    /// 累计回收调用次数。
    pub reclaims: u64,
    /// 累计换出字节数。
    pub swapped: u64,
    /// 累计跨节点迁移字节数。
    pub migrated: u64,
}

impl OomState {
    /// 构造默认状态。
    pub fn new() -> Self {
        Self {
            policy: OomPolicy::default(),
            last: None,
            events: 0,
            kills: 0,
            reclaimed: 0,
            reclaims: 0,
            swapped: 0,
            migrated: 0,
        }
    }

    /// 判断任务是否豁免 OOM。
    pub fn exempt(name: &str) -> bool {
        name == "root"
            || name.starts_with("hk-server-memory")
            || name == "hk-server-oom-observer"
            || name.starts_with("hk-server-debug")
            || name.starts_with("hk-server-checkpoint")
            || name.starts_with("hk-server-taskman")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_conservative_but_enforcing() {
        let policy = OomPolicy::default();
        assert_eq!(policy.min_free_percent, 10);
        assert_eq!(policy.action, 1);
        assert!(OomState::exempt("root"));
        assert!(OomState::exempt("hk-server-memory"));
        assert!(!OomState::exempt("hk-server-console-test"));
    }
}
