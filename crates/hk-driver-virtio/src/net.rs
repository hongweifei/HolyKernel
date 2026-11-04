//! virtio-net 设备封装。
//!
//! 使用 `VirtIONetRaw` 自己管理接收与发送缓冲：接收缓冲初始化时全部入队，
//! 发送只把帧提交到发送队列并通知设备，完成由后续轮询回收。这样发送路径不会
//! 在等待 used-ring 上自旋——设备偶发延迟完成时也不会卡住驱动服务。

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use spin::Mutex;
use virtio_drivers::device::net::VirtIONetRaw;
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::HalImpl;

pub(crate) const QUEUE_SIZE: usize = 16;
pub(crate) const RECEIVE_BUFFER: usize = 4096;

/// 一个已提交到接收队列的缓冲。
struct RxBuffer {
    data: Vec<u8>,
}

/// 一个已提交、等待设备完成的发送缓冲。
struct TxBuffer {
    token: u16,
    data: Vec<u8>,
}

/// 待发队列上限；超过后丢弃新帧，避免无界增长。
const MAX_PENDING: usize = 64;

struct State {
    net: VirtIONetRaw<HalImpl, PciTransport, QUEUE_SIZE>,
    rx: Vec<Option<RxBuffer>>,
    tx: VecDeque<TxBuffer>,
    pending: VecDeque<Vec<u8>>,
}

/// virtio 网络设备。
pub struct Network {
    state: Mutex<State>,
}

impl Network {
    /// 构造网络设备，预先把接收缓冲全部投入接收队列。
    pub(crate) fn new(
        mut net: VirtIONetRaw<HalImpl, PciTransport, QUEUE_SIZE>,
    ) -> Result<Self, virtio_drivers::Error> {
        let mut rx: Vec<Option<RxBuffer>> = (0..QUEUE_SIZE).map(|_| None).collect();
        for _ in 0..QUEUE_SIZE {
            let mut data = vec![0_u8; RECEIVE_BUFFER];
            // SAFETY: 缓冲交给设备后不再访问，直到 `receive_complete`。
            let token = unsafe { net.receive_begin(&mut data) }?;
            rx[usize::from(token)] = Some(RxBuffer { data });
        }
        Ok(Self {
            state: Mutex::new(State {
                net,
                rx,
                tx: VecDeque::new(),
                pending: VecDeque::new(),
            }),
        })
    }

    /// 接收一帧以太网数据；队列为空时返回 `None`。
    pub fn receive(&self) -> Option<Vec<u8>> {
        let mut state = self.state.lock();
        let token = state.net.poll_receive()?;
        let index = usize::from(token);
        let mut buffer = state.rx.get_mut(index)?.take()?;
        // SAFETY: `token` 是 `receive_begin` 返回、尚未完成的同一缓冲。
        let result = unsafe { state.net.receive_complete(token, &mut buffer.data) };
        let (header, length) = match result {
            Ok(value) => value,
            Err(_) => {
                // 无法解析时把缓冲重新投入队列，避免永久丢失。
                Self::recycle(&mut state, buffer);
                return None;
            }
        };
        let Some(end) = header.checked_add(length) else {
            Self::recycle(&mut state, buffer);
            return None;
        };
        let Some(packet) = buffer.data.get(header..end).map(|bytes| bytes.to_vec()) else {
            Self::recycle(&mut state, buffer);
            return None;
        };
        Self::recycle(&mut state, buffer);
        Some(packet)
    }

    /// 把接收缓冲重新投入接收队列。
    fn recycle(state: &mut State, mut buffer: RxBuffer) {
        // SAFETY: 缓冲已完成或被丢弃，重新提交后不再访问，直到下次完成。
        if let Ok(token) = unsafe { state.net.receive_begin(&mut buffer.data) }
            && let Some(slot) = state.rx.get_mut(usize::from(token))
        {
            *slot = Some(buffer);
        }
    }

    /// 发送一帧以太网数据；待发队列已满时返回 `false`。
    pub fn send(&self, packet: &[u8]) -> bool {
        let mut state = self.state.lock();
        if state.pending.len() >= MAX_PENDING {
            return false;
        }
        state.pending.push_back(packet.to_vec());
        Self::flush_pending(&mut state);
        true
    }

    /// 回收发送完成并补发待发帧。
    pub fn flush(&self) {
        let mut state = self.state.lock();
        Self::flush_pending(&mut state);
    }

    /// 把待发队列中的帧尽量提交到设备；队列满时留待下次。
    fn flush_pending(state: &mut State) {
        Self::collect_tx(state);
        while state.net.can_send() {
            let Some(packet) = state.pending.pop_front() else {
                break;
            };
            let mut data = vec![0_u8; packet.len() + 64];
            let Ok(header) = state.net.fill_buffer_header(&mut data) else {
                continue;
            };
            if data.len() < header + packet.len() {
                continue;
            }
            data.truncate(header + packet.len());
            data[header..].copy_from_slice(&packet);
            // SAFETY: 缓冲移入 `state.tx`，直到 `transmit_complete` 前都不再访问。
            match unsafe { state.net.transmit_begin(&data) } {
                Ok(token) => state.tx.push_back(TxBuffer { token, data }),
                Err(_) => {
                    state.pending.push_front(packet);
                    break;
                }
            }
        }
    }

    /// 回收已完成的发送缓冲。
    fn collect_tx(state: &mut State) {
        while let Some(token) = state.net.poll_transmit() {
            let Some(index) = state.tx.iter().position(|entry| entry.token == token) else {
                break;
            };
            let buffer = state.tx.remove(index).expect("索引来自 iterator");
            // SAFETY: `buffer.data` 正是 `transmit_begin` 时提交的同一缓冲。
            let _ = unsafe { state.net.transmit_complete(token, &buffer.data) };
        }
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&self) {
        self.state.lock().net.ack_interrupt();
    }

    /// 启用设备中断。
    pub fn enable_interrupts(&self) {
        self.state.lock().net.enable_interrupts();
    }

    /// 禁用设备中断。
    pub fn disable_interrupts(&self) {
        self.state.lock().net.disable_interrupts();
    }

    /// 返回 MAC 地址。
    pub fn mac_address(&self) -> [u8; 6] {
        self.state.lock().net.mac_address()
    }

    /// 是否还能接收。
    pub fn can_receive(&self) -> bool {
        self.state.lock().net.poll_receive().is_some()
    }

    /// 是否还能发送。
    pub fn can_send(&self) -> bool {
        self.state.lock().net.can_send()
    }

    /// 驱动接收缓冲长度。
    pub const fn receive_buffer_len(&self) -> usize {
        RECEIVE_BUFFER
    }
}
