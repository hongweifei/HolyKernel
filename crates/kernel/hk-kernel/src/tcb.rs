//! TCB、线程状态与调用回复。

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use hk_cadence::CadenceClass;
use hk_handle::{HandleTable, ObjectId};
use hk_ipc::Message;

use crate::task::TaskId;

/// 线程编号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TcbId(u64);

impl TcbId {
    /// 以编号构造线程标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回编号。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 线程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    /// 已创建，待启动或已退出。
    Inactive,
    /// 在就绪队列中。
    Runnable,
    /// 正在 CPU 上执行。
    Running,
    /// 等待 IPC 或通知。
    Blocked,
}

/// 阻塞原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// 等待端点接收者。
    Send {
        /// 目标端点。
        endpoint: ObjectId,
    },
    /// 等待端点消息。
    Recv {
        /// 目标端点。
        endpoint: ObjectId,
    },
    /// 调用后等待回复。
    Call {
        /// 目标端点。
        endpoint: ObjectId,
    },
    /// 等待通知位。
    Notification {
        /// 目标通知。
        notification: ObjectId,
        /// 等待的位掩码。
        mask: u64,
    },
    /// 由用户态运行库挂起，等待运行库唤醒。
    Park,
}

/// 待回复的调用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCall {
    /// 调用目标端点。
    pub endpoint: ObjectId,
    /// 调用序号，用于校验回复令牌。
    pub sequence: u64,
    /// 服务端回复的消息。
    pub reply: Option<Message>,
    /// 是否已回复，防止同一调用被重复回复。
    pub answered: bool,
}

/// 回复令牌：服务端用它把结果送回调用方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyToken {
    pub(crate) caller: TcbId,
    pub(crate) sequence: u64,
}

impl ReplyToken {
    /// 返回调用方线程。
    pub const fn caller(self) -> TcbId {
        self.caller
    }

    /// 返回调用序号。
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

/// 已交付、等待接收方取走的消息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDelivery {
    /// 发送方线程。
    pub sender: TcbId,
    /// 消息本体。
    pub message: Message,
    /// 调用回复令牌；普通发送为 `None`。
    pub reply: Option<ReplyToken>,
}

/// 一次 caller-pays 借贷。
///
/// 服务端 TCB 在 `call` 期间借用调用方契约：类别、队列键与时间片取两者中更强
/// 的一个，执行时间同时记入调用方 Meter 与预算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LendingFrame {
    /// 调用方 Task。
    pub caller: TaskId,
    /// 调用序号。
    pub sequence: u64,
    /// 调用方类别。
    pub class: CadenceClass,
    /// 调用方队列键。
    pub key: u64,
    /// 调用方时间片（契约 tick）。
    pub slice: u64,
}

/// 线程控制块。
pub struct Tcb {
    id: TcbId,
    task: TaskId,
    state: ThreadState,
    handles: HandleTable,
    blocked: Option<BlockReason>,
    pending_call: Option<PendingCall>,
    deliveries: VecDeque<PendingDelivery>,
    lending: Vec<LendingFrame>,
    sequence: u64,
}

impl Tcb {
    /// 创建线程控制块，初始状态为就绪。
    pub fn new(id: TcbId, task: TaskId, handles: HandleTable) -> Self {
        Self {
            id,
            task,
            state: ThreadState::Runnable,
            handles,
            blocked: None,
            pending_call: None,
            deliveries: VecDeque::new(),
            lending: Vec::new(),
            sequence: 0,
        }
    }

    /// 返回线程编号。
    pub fn id(&self) -> TcbId {
        self.id
    }

    /// 返回所属 Task。
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// 返回线程状态。
    pub fn state(&self) -> ThreadState {
        self.state
    }

    /// 返回句柄表。
    pub fn handles(&self) -> &HandleTable {
        &self.handles
    }

    /// 返回句柄表可变引用。
    pub fn handles_mut(&mut self) -> &mut HandleTable {
        &mut self.handles
    }

    /// 返回阻塞原因。
    pub fn blocked(&self) -> Option<BlockReason> {
        self.blocked
    }

    /// 返回待回复调用。
    pub fn pending_call(&self) -> Option<&PendingCall> {
        self.pending_call.as_ref()
    }

    /// 返回待回复调用可变引用。
    pub fn pending_call_mut(&mut self) -> Option<&mut PendingCall> {
        self.pending_call.as_mut()
    }

    /// 取走待回复消息；取走后清除调用记录。
    pub fn take_reply(&mut self) -> Option<Message> {
        let reply = self.pending_call.as_mut()?.reply.take()?;
        self.pending_call = None;
        Some(reply)
    }

    /// 判断是否有待取消息。
    pub fn has_delivery(&self) -> bool {
        !self.deliveries.is_empty()
    }

    /// 返回待取消息数。
    pub fn pending_deliveries(&self) -> usize {
        self.deliveries.len()
    }

    /// 取走最早的一条待取消息。
    pub fn take_delivery(&mut self) -> Option<PendingDelivery> {
        self.deliveries.pop_front()
    }

    /// 取走指定发送者的最早一条待取消息；`None` 表示任意发送者。
    pub fn take_delivery_from(&mut self, expected: Option<TcbId>) -> Option<PendingDelivery> {
        match expected {
            None => self.deliveries.pop_front(),
            Some(expected) => {
                let index = self
                    .deliveries
                    .iter()
                    .position(|delivery| delivery.sender == expected)?;
                self.deliveries.remove(index)
            }
        }
    }

    /// 追加一条待取消息。
    pub fn push_delivery(&mut self, delivery: PendingDelivery) {
        self.deliveries.push_back(delivery);
    }

    /// 分配下一个调用序号。
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// 压入一次借贷；超过深度上限时返回 `false`。
    pub fn push_lending(&mut self, frame: LendingFrame, depth: u64) -> bool {
        if depth == 0 || self.lending.len() as u64 >= depth {
            return false;
        }
        self.lending.push(frame);
        true
    }

    /// 按调用方与序号弹出借贷。
    pub fn pop_lending(&mut self, caller: TaskId, sequence: u64) -> bool {
        let Some(index) = self
            .lending
            .iter()
            .position(|frame| frame.caller == caller && frame.sequence == sequence)
        else {
            return false;
        };
        self.lending.remove(index);
        true
    }

    /// 移除某个调用方的全部借贷；返回移除数量。
    pub fn remove_lending_for(&mut self, caller: TaskId) -> usize {
        let before = self.lending.len();
        self.lending.retain(|frame| frame.caller != caller);
        before - self.lending.len()
    }

    /// 返回最强借贷：类别优先，其次队列键。
    pub fn strongest_lending(&self) -> Option<LendingFrame> {
        self.lending
            .iter()
            .copied()
            .min_by_key(|frame| (frame.class, frame.key))
    }

    /// 返回当前借贷数量。
    pub fn lending_depth(&self) -> usize {
        self.lending.len()
    }

    /// 返回当前借贷栈。
    pub fn lending(&self) -> &[LendingFrame] {
        &self.lending
    }

    pub(crate) fn set_state(&mut self, state: ThreadState) {
        self.state = state;
    }

    pub(crate) fn set_blocked(&mut self, reason: Option<BlockReason>) {
        self.blocked = reason;
    }

    pub(crate) fn set_pending_call(&mut self, call: Option<PendingCall>) {
        self.pending_call = call;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_increasing_call_sequences() {
        let mut tcb = Tcb::new(TcbId::new(1), TaskId::new(1), HandleTable::new(8));

        assert_eq!(tcb.next_sequence(), 1);
        assert_eq!(tcb.next_sequence(), 2);
    }

    #[test]
    fn take_reply_clears_pending_call() {
        let mut tcb = Tcb::new(TcbId::new(1), TaskId::new(1), HandleTable::new(8));
        tcb.set_pending_call(Some(PendingCall {
            endpoint: ObjectId::new(3),
            sequence: 1,
            reply: Some(Message::new(42)),
            answered: true,
        }));

        assert_eq!(tcb.take_reply(), Some(Message::new(42)));
        assert!(tcb.pending_call().is_none());
        assert_eq!(tcb.take_reply(), None);
    }
}
