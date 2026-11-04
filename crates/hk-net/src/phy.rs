//! 网络帧队列与 smoltcp PHY 适配。

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

const MAX_QUEUE: usize = 64;
const MTU: usize = 1514;

/// 双向以太网帧队列；RX 由驱动/Net Endpoint 填充，TX 由协议栈产生。
#[derive(Default)]
pub struct FrameQueue {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}

impl FrameQueue {
    /// 构造空队列。
    pub const fn new() -> Self {
        Self {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
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
    }
}

impl Device for FrameQueue {
    type RxToken<'a> = QueueRxToken;
    type TxToken<'a> = QueueTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let frame = self.rx.pop_front()?;
        Some((QueueRxToken(frame), QueueTxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.tx.len() >= MAX_QUEUE {
            return None;
        }
        Some(QueueTxToken(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = MTU;
        capabilities.max_burst_size = Some(1);
        capabilities
    }
}

/// 接收 token。
pub struct QueueRxToken(Vec<u8>);

impl RxToken for QueueRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

/// 发送 token。
pub struct QueueTxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl TxToken for QueueTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut frame = vec![0_u8; len];
        let result = f(&mut frame);
        self.0.push_back(frame);
        result
    }
}
