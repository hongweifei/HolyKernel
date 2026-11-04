//! virtio-net 设备封装。
//!
//! 当前封装复用 `virtio-drivers` 的缓冲管理接口；后续接入共享 Frame 时
//! 可替换为 `VirtIONetRaw`，协议栈接口保持不变。

use alloc::vec::Vec;

use spin::Mutex;
use virtio_drivers::device::net::{TxBuffer, VirtIONet};
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::HalImpl;

const QUEUE_SIZE: usize = 16;
pub(crate) const RECEIVE_BUFFER: usize = 4096;

/// virtio 网络设备。
pub struct Network {
    net: Mutex<VirtIONet<HalImpl, PciTransport, QUEUE_SIZE>>,
}

impl Network {
    pub(crate) fn new(net: VirtIONet<HalImpl, PciTransport, QUEUE_SIZE>) -> Self {
        Self {
            net: Mutex::new(net),
        }
    }

    /// 接收一帧以太网数据；队列为空时返回 `None`。
    pub fn receive(&self) -> Option<Vec<u8>> {
        let mut net = self.net.lock();
        let rx = net.receive().ok()?;
        let packet = rx.packet().to_vec();
        let _ = net.recycle_rx_buffer(rx);
        Some(packet)
    }

    /// 发送一帧以太网数据。
    pub fn send(&self, packet: &[u8]) -> bool {
        self.net.lock().send(TxBuffer::from(packet)).is_ok()
    }

    /// 返回 MAC 地址。
    pub fn mac_address(&self) -> [u8; 6] {
        self.net.lock().mac_address()
    }

    /// 是否还能接收。
    pub fn can_receive(&self) -> bool {
        self.net.lock().can_recv()
    }

    /// 是否还能发送。
    pub fn can_send(&self) -> bool {
        self.net.lock().can_send()
    }

    /// 驱动接收缓冲长度。
    pub const fn receive_buffer_len(&self) -> usize {
        RECEIVE_BUFFER
    }
}
