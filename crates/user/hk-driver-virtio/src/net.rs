//! virtio-net 设备封装。
//!
//! 使用 `VirtIONetRaw` 自己管理接收与发送缓冲：接收缓冲初始化时全部入队，
//! 发送只把帧提交到发送队列并通知设备，完成由后续轮询回收。这样发送路径不会
//! 在等待 used-ring 上自旋——设备偶发延迟完成时也不会卡住驱动服务。

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use hk_proto::net::{NET_SHARED_RX, NET_SHARED_TX};
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
/// 发送帧中为 virtio 头预留的空间；与 `hk-server-api` 的 `NET_TX_HEADER` 一致。
const TX_HEADER: usize = 32;

/// 共享帧环布局。
#[derive(Clone, Copy)]
struct SharedLayout {
    rx: [u64; NET_SHARED_RX],
    tx: [u64; NET_SHARED_TX],
    frame_len: usize,
}

struct State {
    net: VirtIONetRaw<HalImpl, PciTransport, QUEUE_SIZE>,
    rx: Vec<Option<RxBuffer>>,
    /// 尚未提交的普通接收缓冲；共享模式下不使用。
    legacy: Vec<RxBuffer>,
    tx: VecDeque<TxBuffer>,
    pending: VecDeque<Vec<u8>>,
    shared: Option<SharedLayout>,
    shared_rx: Vec<Option<usize>>,
    shared_tx: VecDeque<(u16, usize)>,
    /// 设备队列暂时满时留待补发的共享发送帧。
    shared_pending: VecDeque<(usize, usize)>,
}

/// virtio 网络设备。
pub struct Network {
    state: Mutex<State>,
}

impl Network {
    /// 构造网络设备，预先把接收缓冲全部投入接收队列。
    pub(crate) fn new(
        net: VirtIONetRaw<HalImpl, PciTransport, QUEUE_SIZE>,
    ) -> Result<Self, virtio_drivers::Error> {
        let rx: Vec<Option<RxBuffer>> = (0..QUEUE_SIZE).map(|_| None).collect();
        let legacy: Vec<RxBuffer> = (0..QUEUE_SIZE)
            .map(|_| RxBuffer {
                data: vec![0_u8; RECEIVE_BUFFER],
            })
            .collect();
        Ok(Self {
            state: Mutex::new(State {
                net,
                rx,
                legacy,
                tx: VecDeque::new(),
                pending: VecDeque::new(),
                shared: None,
                shared_rx: (0..QUEUE_SIZE).map(|_| None).collect(),
                shared_tx: VecDeque::new(),
                shared_pending: VecDeque::new(),
            }),
        })
    }

    /// 普通模式：把预置接收缓冲全部投入接收队列。
    pub fn enable_legacy_rx(&self) {
        let mut state = self.state.lock();
        while let Some(mut buffer) = state.legacy.pop() {
            // SAFETY: 缓冲交给设备后不再访问，直到 `receive_complete`。
            match unsafe { state.net.receive_begin(&mut buffer.data) } {
                Ok(token) => {
                    if let Some(slot) = state.rx.get_mut(usize::from(token)) {
                        *slot = Some(buffer);
                    }
                }
                Err(_) => {
                    state.legacy.push(buffer);
                    break;
                }
            }
        }
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

    /// 回收发送完成并补发待发帧；共享模式下由服务轮询完成。
    pub fn flush(&self) {
        let mut state = self.state.lock();
        if state.shared.is_none() {
            Self::flush_pending(&mut state);
        }
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

    /// 设置共享帧环布局。
    pub fn set_shared_layout(
        &self,
        rx: [u64; NET_SHARED_RX],
        tx: [u64; NET_SHARED_TX],
        frame_len: usize,
    ) {
        self.state.lock().shared = Some(SharedLayout { rx, tx, frame_len });
    }

    /// 把一个共享帧提交为接收缓冲。
    pub fn submit_shared_rx(&self, index: usize) -> bool {
        let mut state = self.state.lock();
        Self::submit_shared_rx_locked(&mut state, index)
    }

    fn submit_shared_rx_locked(state: &mut State, index: usize) -> bool {
        let Some(shared) = state.shared else {
            return false;
        };
        let Some(address) = shared.rx.get(index).copied() else {
            return false;
        };
        // SAFETY: 帧由驱动分配并登记，提交期间不访问，直到完成通知。
        let slice =
            unsafe { core::slice::from_raw_parts_mut(address as *mut u8, shared.frame_len) };
        // SAFETY: 与 `receive_begin` 的约定一致，缓冲在完成前保持有效。
        match unsafe { state.net.receive_begin(slice) } {
            Ok(token) => {
                if let Some(slot) = state.shared_rx.get_mut(usize::from(token)) {
                    *slot = Some(index);
                }
                true
            }
            Err(_) => false,
        }
    }

    /// 取走一个已完成的共享接收帧，返回帧索引、头偏移与长度。
    pub fn poll_shared_rx(&self) -> Option<(usize, usize, usize)> {
        let mut state = self.state.lock();
        let shared = state.shared?;
        let token = state.net.poll_receive()?;
        let index = state
            .shared_rx
            .get_mut(usize::from(token))
            .and_then(Option::take)?;
        let address = *shared.rx.get(index)?;
        // SAFETY: 该 token 对应此前提交的共享帧，直到本次完成前未被访问。
        let slice =
            unsafe { core::slice::from_raw_parts_mut(address as *mut u8, shared.frame_len) };
        // SAFETY: `token` 是同一帧的完成 token。
        match unsafe { state.net.receive_complete(token, slice) } {
            Ok((header, length)) => Some((index, header, length)),
            Err(_) => {
                // 解析失败时重新提交，避免帧永久丢失。
                Self::submit_shared_rx_locked(&mut state, index);
                None
            }
        }
    }

    /// 从共享帧发送一帧；`length` 为以太网帧长度，字节位于 `TX_HEADER` 之后。
    ///
    /// 设备队列暂时满时进入待发队列并返回 `true`（已接受）；真正失败返回 `false`。
    pub fn submit_shared_tx(&self, index: usize, length: usize) -> bool {
        let mut state = self.state.lock();
        let Some(shared) = state.shared else {
            return false;
        };
        if length > shared.frame_len.saturating_sub(TX_HEADER) {
            return false;
        }
        if !state.net.can_send() {
            state.shared_pending.push_back((index, length));
            return true;
        }
        Self::transmit_shared_locked(&mut state, index, length)
    }

    fn transmit_shared_locked(state: &mut State, index: usize, length: usize) -> bool {
        let Some(shared) = state.shared else {
            return false;
        };
        let Some(address) = shared.tx.get(index).copied() else {
            return false;
        };
        // SAFETY: 帧由本侧持有并独占写入，直到 `transmit_complete`。
        let slice =
            unsafe { core::slice::from_raw_parts_mut(address as *mut u8, shared.frame_len) };
        let Ok(header) = state.net.fill_buffer_header(slice) else {
            return false;
        };
        if header != TX_HEADER {
            slice.copy_within(TX_HEADER..TX_HEADER + length, header);
        }
        // SAFETY: 缓冲移入 `shared_tx`，直到 `transmit_complete` 前不再访问。
        match unsafe { state.net.transmit_begin(&slice[..header + length]) } {
            Ok(token) => {
                state.shared_tx.push_back((token, index));
                true
            }
            Err(_) => false,
        }
    }

    /// 取走一个发送完成的共享帧索引。
    pub fn poll_shared_tx(&self) -> Option<usize> {
        let mut state = self.state.lock();
        let shared = state.shared?;
        while state.net.can_send() {
            let Some((index, length)) = state.shared_pending.pop_front() else {
                break;
            };
            if !Self::transmit_shared_locked(&mut state, index, length) {
                state.shared_pending.push_front((index, length));
                break;
            }
        }
        let token = state.net.poll_transmit()?;
        let position = state
            .shared_tx
            .iter()
            .position(|(entry, _)| *entry == token)?;
        let (_, index) = state.shared_tx.remove(position)?;
        let address = *shared.tx.get(index)?;
        // SAFETY: 该 token 对应此前提交的共享帧。
        let slice =
            unsafe { core::slice::from_raw_parts_mut(address as *mut u8, shared.frame_len) };
        // SAFETY: `token` 是同一帧的完成 token。
        let _ = unsafe { state.net.transmit_complete(token, slice) };
        Some(index)
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
