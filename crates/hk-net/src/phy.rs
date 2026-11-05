//! 网络帧队列与 smoltcp PHY 适配。
//!
//! 支持两种模式：
//! - 普通队列：驱动通过消息载荷把帧复制进来；
//! - 共享环：驱动与协议栈共享一组 Frame，协议栈直接读写映射内存，避免逐包复制。

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

const MAX_QUEUE: usize = 64;
const MTU: usize = 1514;
/// 发送帧中为 virtio 头预留的空间；与 `hk-server-api` 的 `NET_TX_HEADER` 一致。
const TX_HEADER: usize = 32;

/// 一个待处理的接收帧描述。
#[derive(Clone, Copy)]
struct RxDesc {
    index: usize,
    offset: usize,
    len: usize,
}

/// 共享环状态。
struct SharedQueue {
    rx_base: u64,
    tx_base: u64,
    frame_len: usize,
    rx_ready: VecDeque<RxDesc>,
    rx_consumed: Vec<usize>,
    tx_free: VecDeque<usize>,
    tx_pending: Vec<(usize, usize)>,
}

/// 双向以太网帧队列；RX 由驱动/Net Endpoint 填充，TX 由协议栈产生。
#[derive(Default)]
pub struct FrameQueue {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
    shared: Option<SharedQueue>,
}

impl FrameQueue {
    /// 构造空队列。
    pub const fn new() -> Self {
        Self {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            shared: None,
        }
    }

    /// 启用共享帧环；`tx_count` 个发送帧初始可用。
    pub fn enable_shared(&mut self, rx_base: u64, tx_base: u64, frame_len: usize, tx_count: usize) {
        let mut tx_free = VecDeque::new();
        for index in 0..tx_count {
            tx_free.push_back(index);
        }
        self.shared = Some(SharedQueue {
            rx_base,
            tx_base,
            frame_len,
            rx_ready: VecDeque::new(),
            rx_consumed: Vec::new(),
            tx_free,
            tx_pending: Vec::new(),
        });
    }

    /// 借用共享接收帧的一小段视图（用于 ARP 观察等旁路处理）。
    pub fn shared_slice(&self, index: usize, offset: usize, length: usize) -> Option<&[u8]> {
        let shared = self.shared.as_ref()?;
        if offset.saturating_add(length) > shared.frame_len {
            return None;
        }
        let address = shared.rx_base + (index * shared.frame_len + offset) as u64;
        // SAFETY: 描述由驱动在完成通知中给出，归还前该帧只读；长度已校验。
        Some(unsafe { core::slice::from_raw_parts(address as *const u8, length) })
    }

    /// 注入一个共享接收帧描述。
    pub fn push_shared_rx(&mut self, index: usize, offset: usize, length: usize) {
        if let Some(shared) = &mut self.shared
            && length <= shared.frame_len
        {
            shared.rx_ready.push_back(RxDesc {
                index,
                offset,
                len: length,
            });
        }
    }

    /// 取走协议栈已消费的接收帧与待发送帧。
    pub fn take_shared_events(&mut self, rx: &mut Vec<usize>, tx: &mut Vec<(usize, usize)>) {
        if let Some(shared) = &mut self.shared {
            rx.append(&mut shared.rx_consumed);
            tx.append(&mut shared.tx_pending);
        }
    }

    /// 归还一个发送完成的共享帧。
    pub fn shared_tx_done(&mut self, index: usize) {
        if let Some(shared) = &mut self.shared {
            shared.tx_free.push_back(index);
        }
    }

    /// 注入一帧到接收队列；返回是否成功。
    pub fn push_rx(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() || frame.len() > MTU || self.rx.len() >= MAX_QUEUE {
            return false;
        }
        self.rx.push_back(frame.to_vec());
        true
    }

    /// 取出一帧待发送数据。
    pub fn pop_tx(&mut self) -> Option<Vec<u8>> {
        self.tx.pop_front()
    }

    /// 是否还有待发送帧。
    pub fn has_tx(&self) -> bool {
        !self.tx.is_empty()
    }

    /// 清空队列。
    pub fn clear(&mut self) {
        self.rx.clear();
        self.tx.clear();
        self.shared = None;
    }
}

impl Device for FrameQueue {
    type RxToken<'a> = QueueRxToken<'a>;
    type TxToken<'a> = QueueTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if self.shared.is_some() {
            return self.receive_shared();
        }
        let frame = self.rx.pop_front()?;
        Some((QueueRxToken::Owned(frame), QueueTxToken::Vec(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.shared.is_some() {
            let shared = self.shared.as_mut()?;
            // 只查看队首，真正占用留到 `consume`；未使用的 token 不会泄漏帧。
            let index = *shared.tx_free.front()?;
            return Some(QueueTxToken::Shared(SharedTxToken {
                base: shared.tx_base,
                frame_len: shared.frame_len,
                index,
                free: &mut shared.tx_free,
                pending: &mut shared.tx_pending,
            }));
        }
        if self.tx.len() >= MAX_QUEUE {
            return None;
        }
        Some(QueueTxToken::Vec(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = MTU;
        capabilities.max_burst_size = Some(1);
        capabilities
    }
}

impl FrameQueue {
    fn receive_shared(&mut self) -> Option<(QueueRxToken<'_>, QueueTxToken<'_>)> {
        let shared = self.shared.as_mut()?;
        // 没有空闲发送帧时先不取走接收帧，避免丢包。
        let index = *shared.tx_free.front()?;
        let desc = shared.rx_ready.pop_front()?;
        let rx = QueueRxToken::Shared(SharedRxToken {
            base: shared.rx_base,
            frame_len: shared.frame_len,
            desc,
            consumed: &mut shared.rx_consumed,
        });
        let tx = QueueTxToken::Shared(SharedTxToken {
            base: shared.tx_base,
            frame_len: shared.frame_len,
            index,
            free: &mut shared.tx_free,
            pending: &mut shared.tx_pending,
        });
        Some((rx, tx))
    }
}

/// 接收 token。
pub enum QueueRxToken<'a> {
    /// 普通队列中的拥有型帧。
    Owned(Vec<u8>),
    /// 共享环中的借用帧。
    Shared(SharedRxToken<'a>),
}

impl RxToken for QueueRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        match self {
            Self::Owned(frame) => f(&frame),
            Self::Shared(token) => token.consume(f),
        }
    }
}

/// 发送 token。
pub enum QueueTxToken<'a> {
    /// 普通队列。
    Vec(&'a mut VecDeque<Vec<u8>>),
    /// 共享环。
    Shared(SharedTxToken<'a>),
}

impl TxToken for QueueTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        match self {
            Self::Vec(queue) => {
                let mut frame = vec![0_u8; len];
                let result = f(&mut frame);
                queue.push_back(frame);
                result
            }
            Self::Shared(token) => token.consume(len, f),
        }
    }
}

/// 共享接收 token。
pub struct SharedRxToken<'a> {
    base: u64,
    frame_len: usize,
    desc: RxDesc,
    consumed: &'a mut Vec<usize>,
}

impl SharedRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let address = self.base + (self.desc.index * self.frame_len + self.desc.offset) as u64;
        // SAFETY: 该帧由驱动注册并与协议栈共享；描述来自驱动的完成通知，
        // 在归还前协议栈独占读取。
        let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, self.desc.len) };
        f(bytes)
    }
}

impl Drop for SharedRxToken<'_> {
    fn drop(&mut self) {
        // 无论是否被 consume，都要把帧归还给驱动，避免接收帧泄漏。
        self.consumed.push(self.desc.index);
    }
}

/// 共享发送 token。
pub struct SharedTxToken<'a> {
    base: u64,
    frame_len: usize,
    index: usize,
    free: &'a mut VecDeque<usize>,
    pending: &'a mut Vec<(usize, usize)>,
}

impl SharedTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // 到这里才真正占用发送帧；`transmit()` 只是窥视队首。
        if self.free.front().copied() == Some(self.index) {
            self.free.pop_front();
        }
        let address = self.base + (self.index * self.frame_len + TX_HEADER) as u64;
        // SAFETY: 该帧已从空闲表取出，归还前独占写入；`TX_HEADER` 之外
        // 的空间由协议栈使用，驱动只在收到发送消息后读取。
        let bytes = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) };
        let result = f(bytes);
        self.pending.push((self.index, len));
        result
    }
}
