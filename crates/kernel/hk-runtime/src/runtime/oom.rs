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

/// 事件类别：回收。
pub const MEMORY_EVENT_RECLAIM: u8 = 0;
/// 事件类别：Provider 换出。
pub const MEMORY_EVENT_SWAP: u8 = 1;
/// 事件类别：OOM。
pub const MEMORY_EVENT_OOM: u8 = 2;
/// 事件类别：NUMA 迁移。
pub const MEMORY_EVENT_MIGRATE: u8 = 3;

/// 事件环容量。
pub const MEMORY_EVENT_CAPACITY: usize = 64;

/// 一条内存审计事件。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryEvent {
    /// 全局递增序号。
    pub sequence: u64,
    /// `MEMORY_EVENT_*`。
    pub kind: u8,
    /// 能力对象标识。
    pub object: u64,
    /// 页在能力内的字节偏移。
    pub offset: u64,
    /// 事件字节数。
    pub length: u64,
    /// 任务槽位。
    pub task: u64,
    /// 目标节点、空闲百分比或结果。
    pub value: u64,
}

/// OOM 状态。
#[derive(Clone, Debug)]
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
    /// 事件环。
    pub memory_events: [MemoryEvent; MEMORY_EVENT_CAPACITY],
    /// 下一个写入位置。
    pub memory_event_next: usize,
    /// 下一个事件序号。
    pub memory_event_sequence: u64,
    /// 已记录事件数。
    pub memory_event_count: usize,
    /// 订阅事件通知的对象标识。
    pub event_notification: Option<u64>,
}

impl Default for OomState {
    fn default() -> Self {
        Self::new()
    }
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
            memory_events: [MemoryEvent::default(); MEMORY_EVENT_CAPACITY],
            memory_event_next: 0,
            memory_event_sequence: 1,
            memory_event_count: 0,
            event_notification: None,
        }
    }

    /// 记录一条内存审计事件；环形覆盖最旧记录。
    pub fn record_event(
        &mut self,
        kind: u8,
        object: u64,
        offset: u64,
        length: u64,
        task: u64,
        value: u64,
    ) {
        let sequence = self.memory_event_sequence.max(1);
        self.memory_event_sequence = sequence.saturating_add(1);
        self.memory_events[self.memory_event_next] = MemoryEvent {
            sequence,
            kind,
            object,
            offset,
            length,
            task,
            value,
        };
        self.memory_event_next = (self.memory_event_next + 1) % MEMORY_EVENT_CAPACITY;
        self.memory_event_count = (self.memory_event_count + 1).min(MEMORY_EVENT_CAPACITY);
    }

    /// 按时间顺序返回事件，最旧的在前。
    pub fn events(&self) -> impl Iterator<Item = &MemoryEvent> {
        let start = self.memory_event_next;
        (0..MEMORY_EVENT_CAPACITY).filter_map(move |offset| {
            let event = &self.memory_events[(start + offset) % MEMORY_EVENT_CAPACITY];
            (event.sequence != 0).then_some(event)
        })
    }

    /// 判断任务是否豁免 OOM。
    pub fn exempt(name: &str) -> bool {
        hk_catalog::oom_exempt(name)
    }
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
mod tests {
    use super::*;

    #[test]
    fn memory_event_ring_keeps_newest_in_order() {
        let mut state = OomState::new();
        for index in 0..(MEMORY_EVENT_CAPACITY + 4) {
            state.record_event(MEMORY_EVENT_RECLAIM, index as u64, 0, 4096, 1, 0);
        }
        assert_eq!(state.memory_event_count, MEMORY_EVENT_CAPACITY);
        let sequences: alloc::vec::Vec<_> = state.events().map(|event| event.sequence).collect();
        assert_eq!(sequences.len(), MEMORY_EVENT_CAPACITY);
        assert_eq!(sequences.first(), Some(&5));
        assert_eq!(sequences.last(), Some(&(MEMORY_EVENT_CAPACITY as u64 + 4)));
    }

    #[test]
    fn default_policy_is_conservative_but_enforcing() {
        let policy = OomPolicy::default();
        assert_eq!(policy.min_free_percent, 10);
        assert_eq!(policy.action, 1);
        assert!(OomState::exempt("root"));
        assert!(OomState::exempt("hk-server-memory"));
        assert!(!OomState::exempt("hk-client-console-test"));
    }
}
