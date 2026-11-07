//! 同步 IPC 端点与异步通知。
//!
//! 端点只负责消息排队与交付顺序；阻塞、唤醒与句柄转移由内核执行。等待者用
//! [`Waiter`] 标识，内核把 TCB 编号映射到该标识。

#![no_std]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_handle::Handle;
use spin::Mutex;

/// 消息携带的最大字数。
pub const MAX_MESSAGE_WORDS: usize = 8;

/// 消息携带的最大句柄数。
pub const MAX_MESSAGE_HANDLES: usize = 4;

/// 等待者标识：内核 TCB 的不透明编号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Waiter(u64);

impl Waiter {
    /// 以编号构造等待者标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回编号。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// IPC 消息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// 消息标签，服务端据此分派请求。
    pub label: u64,
    /// 消息字。
    pub words: [u64; MAX_MESSAGE_WORDS],
    /// 字节载荷。
    pub data: Vec<u8>,
    /// 随消息转移的句柄。
    pub handles: [Option<Handle>; MAX_MESSAGE_HANDLES],
}

impl Message {
    /// 以标签构造空消息。
    pub const fn new(label: u64) -> Self {
        Self {
            label,
            words: [0; MAX_MESSAGE_WORDS],
            data: Vec::new(),
            handles: [None; MAX_MESSAGE_HANDLES],
        }
    }

    /// 设置消息字。
    pub fn with_words(mut self, words: [u64; MAX_MESSAGE_WORDS]) -> Self {
        self.words = words;
        self
    }

    /// 设置字节载荷。
    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.data = data;
        self
    }

    /// 设置随消息转移的句柄。
    pub fn with_handles(mut self, handles: [Option<Handle>; MAX_MESSAGE_HANDLES]) -> Self {
        self.handles = handles;
        self
    }
}

/// 每个 Endpoint 最多缓存的待接收消息数。
///
/// 异步发送不会阻塞发送者；超过上限时 [`Endpoint::send`] 返回
/// [`SendOutcome::Full`]，由内核把错误回传给调用方，避免慢消费者导致内存
/// 无界增长。
pub const ENDPOINT_QUEUE_CAPACITY: usize = 64;

/// 发送结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// 已有接收者在等待，消息直接交付。
    Delivered {
        /// 接收者在等待队列中的位置；交付失败时用于原样恢复。
        position: usize,
        /// 接收者。
        receiver: Waiter,
        /// 接收者匹配的发送者。
        expected: Option<Waiter>,
        /// 交付的消息。
        message: Message,
    },
    /// 没有接收者，消息进入有界队列。
    Queued,
    /// 队列已满，消息被拒绝。
    Full,
}

struct PendingSend {
    sender: Waiter,
    message: Message,
}

struct PendingRecv {
    receiver: Waiter,
    expected: Option<Waiter>,
}

#[derive(Default)]
struct EndpointState {
    receivers: VecDeque<PendingRecv>,
    senders: VecDeque<PendingSend>,
}

/// 同步 IPC 端点。
///
/// 发送与接收按 FIFO 配对；端点可携带 badge，用于服务端识别客户端。
pub struct Endpoint {
    badge: Option<u64>,
    state: Mutex<EndpointState>,
}

impl Endpoint {
    /// 构造不带 badge 的端点。
    pub const fn new() -> Self {
        Self {
            badge: None,
            state: Mutex::new(EndpointState {
                receivers: VecDeque::new(),
                senders: VecDeque::new(),
            }),
        }
    }

    /// 构造带 badge 的端点。
    pub const fn with_badge(badge: u64) -> Self {
        Self {
            badge: Some(badge),
            state: Mutex::new(EndpointState {
                receivers: VecDeque::new(),
                senders: VecDeque::new(),
            }),
        }
    }

    /// 返回端点 badge。
    pub fn badge(&self) -> Option<u64> {
        self.badge
    }

    /// 发送消息。
    ///
    /// 优先交付给愿意接收该发送者的等待者；`expected` 为 `None` 的接收者
    /// 接收任意发送者。
    pub fn send(&self, sender: Waiter, message: Message) -> SendOutcome {
        let mut state = self.state.lock();
        let index = state
            .receivers
            .iter()
            .position(|pending| pending.expected.is_none() || pending.expected == Some(sender));
        match index {
            Some(index) => {
                let pending = state.receivers.remove(index).expect("接收者已索引");
                SendOutcome::Delivered {
                    position: index,
                    receiver: pending.receiver,
                    expected: pending.expected,
                    message,
                }
            }
            None if state.senders.len() < ENDPOINT_QUEUE_CAPACITY => {
                state.senders.push_back(PendingSend { sender, message });
                SendOutcome::Queued
            }
            None => SendOutcome::Full,
        }
    }

    /// 接收任意发送者的消息；没有待取消息时把接收者登记为等待者。
    pub fn recv(&self, receiver: Waiter) -> Option<(Waiter, Message)> {
        self.recv_from(receiver, None)
    }

    /// 接收指定发送者的消息；没有匹配消息时把接收者登记为等待者。
    pub fn recv_from(
        &self,
        receiver: Waiter,
        expected: Option<Waiter>,
    ) -> Option<(Waiter, Message)> {
        self.recv_from_indexed(receiver, expected)
            .map(|(_, sender, message)| (sender, message))
    }

    /// 接收消息并返回发送者在队列中的位置，供交付失败时原样恢复。
    pub fn recv_from_indexed(
        &self,
        receiver: Waiter,
        expected: Option<Waiter>,
    ) -> Option<(usize, Waiter, Message)> {
        let mut state = self.state.lock();
        let index = match expected {
            None => (!state.senders.is_empty()).then_some(0),
            Some(expected) => state
                .senders
                .iter()
                .position(|pending| pending.sender == expected),
        };
        if let Some(index) = index {
            let pending = state.senders.remove(index)?;
            return Some((index, pending.sender, pending.message));
        }
        state
            .receivers
            .push_back(PendingRecv { receiver, expected });
        None
    }

    /// 非阻塞接收：没有待取消息时直接返回 `None`，不登记等待者。
    pub fn try_recv_from(&self, expected: Option<Waiter>) -> Option<(Waiter, Message)> {
        self.try_recv_from_indexed(expected)
            .map(|(_, sender, message)| (sender, message))
    }

    /// 非阻塞接收并返回发送者在队列中的位置。
    pub fn try_recv_from_indexed(
        &self,
        expected: Option<Waiter>,
    ) -> Option<(usize, Waiter, Message)> {
        let mut state = self.state.lock();
        let index = match expected {
            None => (!state.senders.is_empty()).then_some(0),
            Some(expected) => state
                .senders
                .iter()
                .position(|pending| pending.sender == expected),
        }?;
        let pending = state.senders.remove(index)?;
        Some((index, pending.sender, pending.message))
    }

    /// 交付失败时把接收者放回原位置。
    pub fn restore_receiver(&self, position: usize, receiver: Waiter, expected: Option<Waiter>) {
        let mut state = self.state.lock();
        let position = position.min(state.receivers.len());
        state
            .receivers
            .insert(position, PendingRecv { receiver, expected });
    }

    /// 交付失败时把发送者消息放回原位置。
    pub fn restore_sender(&self, position: usize, sender: Waiter, message: Message) {
        let mut state = self.state.lock();
        let position = position.min(state.senders.len());
        state
            .senders
            .insert(position, PendingSend { sender, message });
    }

    /// 撤销发送者的排队消息，用于超时与取消。
    pub fn cancel_sender(&self, sender: Waiter) -> bool {
        let mut state = self.state.lock();
        let before = state.senders.len();
        state.senders.retain(|pending| pending.sender != sender);
        state.senders.len() != before
    }

    /// 撤销接收者的等待登记，用于超时与取消。
    pub fn cancel_receiver(&self, receiver: Waiter) -> bool {
        let mut state = self.state.lock();
        let before = state.receivers.len();
        state
            .receivers
            .retain(|pending| pending.receiver != receiver);
        state.receivers.len() != before
    }

    /// 返回等待发送的消息数。
    pub fn waiting_senders(&self) -> usize {
        self.state.lock().senders.len()
    }

    /// 返回等待接收的线程数。
    pub fn waiting_receivers(&self) -> usize {
        self.state.lock().receivers.len()
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Self::new()
    }
}

/// 异步事件通知。
///
/// `signal` 置位并返回需要唤醒的等待者；`wait` 在无待处理位时登记等待。
pub struct Notification {
    pending: AtomicU64,
    waiter: Mutex<Option<Waiter>>,
}

impl Notification {
    /// 构造通知对象。
    pub const fn new() -> Self {
        Self {
            pending: AtomicU64::new(0),
            waiter: Mutex::new(None),
        }
    }

    /// 置位通知并返回被唤醒的等待者。
    pub fn signal(&self, bits: u64) -> Option<Waiter> {
        let mut waiter = self.waiter.lock();
        self.pending.fetch_or(bits, Ordering::AcqRel);
        waiter.take()
    }

    /// 等待通知位；已有待处理位时清除并立即返回。
    pub fn wait(&self, waiter: Waiter, mask: u64) -> Option<u64> {
        let mut slot = self.waiter.lock();
        let pending = self.pending.load(Ordering::Acquire);
        if pending & mask != 0 {
            self.pending.fetch_and(!mask, Ordering::AcqRel);
            return Some(pending & mask);
        }
        *slot = Some(waiter);
        None
    }

    /// 查询待处理位，不清除。
    pub fn poll(&self, mask: u64) -> u64 {
        self.pending.load(Ordering::Acquire) & mask
    }

    /// 非阻塞等待通知位；没有待处理位时返回 `None`。
    pub fn try_wait(&self, mask: u64) -> Option<u64> {
        let _slot = self.waiter.lock();
        let pending = self.pending.load(Ordering::Acquire) & mask;
        if pending == 0 {
            return None;
        }
        self.pending.fetch_and(!mask, Ordering::AcqRel);
        Some(pending)
    }

    /// 清除通知位。
    pub fn clear(&self, bits: u64) {
        self.pending.fetch_and(!bits, Ordering::AcqRel);
    }

    /// 返回全部待处理位。
    pub fn pending(&self) -> u64 {
        self.pending.load(Ordering::Acquire)
    }
}

impl Default for Notification {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiter(value: u64) -> Waiter {
        Waiter::new(value)
    }

    #[test]
    fn queues_messages_until_a_receiver_arrives() {
        let endpoint = Endpoint::new();

        assert_eq!(
            endpoint.send(waiter(1), Message::new(7)),
            SendOutcome::Queued
        );
        assert_eq!(endpoint.waiting_senders(), 1);

        let (sender, message) = endpoint.recv(waiter(2)).expect("收到消息");
        assert_eq!(sender, waiter(1));
        assert_eq!(message.label, 7);
        assert_eq!(endpoint.waiting_senders(), 0);
    }

    #[test]
    fn rejects_send_when_queue_is_full() {
        let endpoint = Endpoint::new();
        for _ in 0..ENDPOINT_QUEUE_CAPACITY {
            assert_eq!(
                endpoint.send(waiter(1), Message::new(1)),
                SendOutcome::Queued
            );
        }
        assert_eq!(endpoint.send(waiter(1), Message::new(2)), SendOutcome::Full);
        assert_eq!(endpoint.waiting_senders(), ENDPOINT_QUEUE_CAPACITY);

        // 取走一条后应重新可以入队。
        assert!(endpoint.recv(waiter(2)).is_some());
        assert_eq!(
            endpoint.send(waiter(1), Message::new(3)),
            SendOutcome::Queued
        );
    }

    #[test]
    fn delivers_to_waiting_receiver() {
        let endpoint = Endpoint::new();

        assert_eq!(endpoint.recv(waiter(2)), None);
        assert_eq!(endpoint.waiting_receivers(), 1);

        let message = Message::new(9).with_words([1, 2, 3, 0, 0, 0, 0, 0]);
        assert_eq!(
            endpoint.send(waiter(1), message.clone()),
            SendOutcome::Delivered {
                position: 0,
                receiver: waiter(2),
                expected: None,
                message
            }
        );
        assert_eq!(endpoint.waiting_receivers(), 0);
    }

    #[test]
    fn recv_from_only_takes_matching_sender() {
        let endpoint = Endpoint::new();
        assert_eq!(endpoint.recv_from(waiter(3), Some(waiter(1))), None);

        let other = Message::new(20);
        assert_eq!(endpoint.send(waiter(2), other.clone()), SendOutcome::Queued);

        let expected = Message::new(10);
        assert_eq!(
            endpoint.send(waiter(1), expected.clone()),
            SendOutcome::Delivered {
                position: 0,
                receiver: waiter(3),
                expected: Some(waiter(1)),
                message: expected,
            }
        );

        let (sender, message) = endpoint
            .recv_from(waiter(4), Some(waiter(2)))
            .expect("匹配发送者");
        assert_eq!(sender, waiter(2));
        assert_eq!(message, other);
    }

    #[test]
    fn keeps_fifo_order() {
        let endpoint = Endpoint::new();
        endpoint.send(waiter(1), Message::new(1));
        endpoint.send(waiter(2), Message::new(2));

        assert_eq!(
            endpoint.recv(waiter(9)).map(|(sender, _)| sender),
            Some(waiter(1))
        );
        assert_eq!(
            endpoint.recv(waiter(9)).map(|(sender, _)| sender),
            Some(waiter(2))
        );
        assert_eq!(endpoint.recv(waiter(9)), None);
    }

    #[test]
    fn cancels_blocked_operations() {
        let endpoint = Endpoint::new();
        endpoint.send(waiter(1), Message::new(1));

        assert!(endpoint.cancel_sender(waiter(1)));
        assert!(!endpoint.cancel_sender(waiter(1)));

        assert_eq!(endpoint.recv(waiter(2)), None);
        assert!(endpoint.cancel_receiver(waiter(2)));
        assert!(!endpoint.cancel_receiver(waiter(2)));

        assert_eq!(endpoint.waiting_senders(), 0);
        assert_eq!(endpoint.waiting_receivers(), 0);
    }

    #[test]
    fn carries_handles_in_messages() {
        let endpoint = Endpoint::new();
        let handle = Handle::new(3, 1);
        let message = Message::new(11).with_handles([Some(handle), None, None, None]);
        endpoint.send(waiter(1), message);

        let (_, received) = endpoint.recv(waiter(2)).expect("收到消息");
        assert_eq!(received.handles[0], Some(handle));
        assert_eq!(received.handles[1..], [None, None, None]);
    }

    #[test]
    fn notification_delivers_pending_bits_immediately() {
        let notification = Notification::new();
        assert_eq!(notification.signal(0b101), None);

        assert_eq!(notification.poll(0b111), 0b101);
        assert_eq!(notification.wait(waiter(1), 0b001), Some(0b001));
        assert_eq!(notification.pending(), 0b100);

        notification.clear(0b100);
        assert_eq!(notification.pending(), 0);
    }

    #[test]
    fn try_receive_does_not_register_waiter() {
        let endpoint = Endpoint::new();
        assert!(endpoint.try_recv_from(Some(waiter(1))).is_none());
        assert_eq!(endpoint.waiting_receivers(), 0);

        assert!(endpoint.recv_from(waiter(1), None).is_none());
        assert_eq!(endpoint.waiting_receivers(), 1);

        // 反复非阻塞轮询不得累积等待者（否则会长期撑大内核堆）。
        let endpoint = Endpoint::new();
        for _ in 0..256 {
            assert!(endpoint.try_recv_from(None).is_none());
        }
        assert_eq!(endpoint.waiting_receivers(), 0);
    }

    #[test]
    fn notification_wakes_registered_waiter() {
        let notification = Notification::new();
        assert_eq!(notification.wait(waiter(4), 0b10), None);

        assert_eq!(notification.signal(0b10), Some(waiter(4)));
        assert_eq!(notification.signal(0b100), None);
        assert_eq!(notification.pending(), 0b110);
    }
}
