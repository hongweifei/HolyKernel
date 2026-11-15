//! virtio-console 设备（多端口）。
//!
//! `virtio-drivers` 的控制台前端只做 port 0；这里自己接管传输层。队列布局按
//! virtio 规范：0/1 = port 0 收发，2/3 = 控制队列，端口 n≥1 为 `2n+2`/`2n+3`。
//! 多端口握手按 QEMU `virtio-serial-bus.c` 的状态机：
//! `DEVICE_READY` → 宿主发 `PORT_ADD` → 每端口 `PORT_READY` → 宿主回
//! `CONSOLE_PORT`/`PORT_NAME` → 每端口 `PORT_OPEN`（置 guest_connected，宿主
//! 才会把 chardev 数据投向该端口的接收队列）。

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use bitflags::bitflags;
use hk_device::DeviceGrant;
use virtio_drivers::PAGE_SIZE;
use virtio_drivers::queue::VirtQueue;
use virtio_drivers::transport::Transport;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::{Error, Result};

use crate::hal::{self, HalImpl};

/// 每个端口的收发队列长度。
const QUEUE_SIZE: usize = 8;
/// 控制队列长度；QEMU 侧为 32。
const CONTROL_QUEUE_SIZE: usize = 8;
/// 端口数量上限：超过该值视为设备异常，按单端口处理。
const MAX_PORTS: u32 = 8;

/// 控制队列索引。
const CONTROL_RX: u16 = 2;
const CONTROL_TX: u16 = 3;

bitflags! {
    /// 控制台特性位。
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    struct Features: u64 {
        const SIZE = 1 << 0;
        const MULTIPORT = 1 << 1;
        const RING_INDIRECT_DESC = 1 << 28;
        const RING_EVENT_IDX = 1 << 29;
        const VERSION_1 = 1 << 32;
    }
}

/// 控制事件（`linux/virtio_console.h`）。
const DEVICE_READY: u16 = 0;
const PORT_ADD: u16 = 1;
const PORT_READY: u16 = 3;
const CONSOLE_PORT: u16 = 4;
const PORT_OPEN: u16 = 6;
const PORT_NAME: u16 = 7;

/// 一个控制台端口的收发状态。
struct Port {
    receive: VirtQueue<HalImpl, QUEUE_SIZE>,
    transmit: VirtQueue<HalImpl, QUEUE_SIZE>,
    buffer: Box<[u8; PAGE_SIZE]>,
    length: usize,
    cursor: usize,
    token: Option<u16>,
    receive_index: u16,
}

impl Port {
    fn new(
        receive: VirtQueue<HalImpl, QUEUE_SIZE>,
        transmit: VirtQueue<HalImpl, QUEUE_SIZE>,
        receive_index: u16,
    ) -> Self {
        Self {
            receive,
            transmit,
            buffer: Box::new([0; PAGE_SIZE]),
            length: 0,
            cursor: 0,
            token: None,
            receive_index,
        }
    }

    /// 把接收缓冲挂到队列上。
    fn arm(&mut self, transport: &mut PciTransport) {
        if self.token.is_some() {
            return;
        }
        let mut outputs: [&mut [u8]; 1] = [self.buffer.as_mut_slice()];
        // SAFETY: 缓冲在令牌回收前保持有效且不被其他代码访问。
        if let Ok(token) = unsafe { self.receive.add(&[], &mut outputs) } {
            if self.receive.should_notify() {
                transport.notify(self.receive_index);
            }
            self.token = Some(token);
        }
    }

    /// 回收已完成的接收，返回本次写入的字节数。
    fn harvest(&mut self, transport: &mut PciTransport) -> usize {
        let Some(token) = self.token else {
            self.arm(transport);
            return 0;
        };
        if !self.receive.can_pop() {
            return 0;
        }
        let mut outputs: [&mut [u8]; 1] = [self.buffer.as_mut_slice()];
        // SAFETY: 该令牌正是 `arm` 提交时的那组缓冲。
        let written = unsafe { self.receive.pop_used(token, &[], &mut outputs) }.unwrap_or(0);
        self.token = None;
        let written = written as usize;
        if written > 0 {
            self.length = written;
            self.cursor = 0;
        }
        self.arm(transport);
        written
    }
}

/// virtio 控制台设备。
pub struct Console {
    transport: PciTransport,
    ports: Vec<Port>,
    control: Option<(
        VirtQueue<HalImpl, CONTROL_QUEUE_SIZE>,
        VirtQueue<HalImpl, CONTROL_QUEUE_SIZE>,
    )>,
    control_buffer: Box<[u8; 128]>,
    control_token: Option<u16>,
    /// 多端口握手状态：已收到 ADD、已回 READY、已发 OPEN。
    added: Vec<bool>,
    ready: Vec<bool>,
    opened: Vec<bool>,
    grant: u64,
    descriptor: DeviceGrant,
    size: Option<(u16, u16)>,
}

impl Console {
    pub(crate) fn new(mut transport: PciTransport) -> Result<Self> {
        let features = Features::SIZE
            | Features::MULTIPORT
            | Features::RING_INDIRECT_DESC
            | Features::RING_EVENT_IDX
            | Features::VERSION_1;
        let negotiated = transport.begin_init(features);
        let multiport = negotiated.contains(Features::MULTIPORT);
        let size = negotiated.contains(Features::SIZE);
        // QEMU 的配置结构始终带 cols/rows，max_nr_ports 在偏移 4；兼容其它实现
        // 时再看偏移 0。
        let max_ports = if multiport {
            let mut found = 1;
            for offset in [4_usize, 0] {
                if let Ok(value) = transport.read_config_space::<u32>(offset)
                    && (2..=MAX_PORTS).contains(&value)
                {
                    found = value;
                    break;
                }
            }
            found
        } else {
            1
        };

        let indirect = negotiated.contains(Features::RING_INDIRECT_DESC);
        let event_idx = negotiated.contains(Features::RING_EVENT_IDX);
        let mut ports = Vec::new();
        for index in 0..max_ports {
            let (rx, tx) = Self::queue_indices(index);
            let receive = VirtQueue::new(&mut transport, rx, indirect, event_idx)?;
            let transmit = VirtQueue::new(&mut transport, tx, indirect, event_idx)?;
            ports.push(Port::new(receive, transmit, rx));
        }
        let control = if multiport {
            let receive = VirtQueue::new(&mut transport, CONTROL_RX, indirect, event_idx)?;
            let transmit = VirtQueue::new(&mut transport, CONTROL_TX, indirect, event_idx)?;
            Some((receive, transmit))
        } else {
            None
        };

        let console_size = if size {
            let columns = transport.read_config_space::<u16>(0).unwrap_or(0);
            let rows = transport.read_config_space::<u16>(2).unwrap_or(0);
            (columns > 0 && rows > 0).then_some((columns, rows))
        } else {
            None
        };

        transport.finish_init();
        let count = ports.len();
        let mut console = Self {
            transport,
            ports,
            control,
            control_buffer: Box::new([0; 128]),
            control_token: None,
            added: vec![false; count],
            ready: vec![false; count],
            opened: vec![false; count],
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
            size: console_size,
        };
        if multiport {
            console.handshake();
        }
        Ok(console)
    }

    /// 端口队列索引：port 0 是 0/1，其余为 `2n+2`/`2n+3`。
    const fn queue_indices(port: u32) -> (u16, u16) {
        if port == 0 {
            (0, 1)
        } else {
            let rx = (port * 2 + 2) as u16;
            (rx, rx + 1)
        }
    }

    /// 多端口握手：声明设备就绪，按宿主事件逐端口 READY / OPEN。
    fn handshake(&mut self) {
        self.arm_control();
        self.control_event(0, DEVICE_READY, 1);
        for _ in 0..20_000_000 {
            self.harvest_control();
            if self.opened.iter().all(|opened| *opened) {
                return;
            }
            core::hint::spin_loop();
        }
        // 兜底：宿主未发 NAME/CONSOLE_PORT 时也把未打开的端口打开。
        for port in 0..self.ports.len() {
            if !self.ready[port] {
                self.control_event(port as u32, PORT_READY, 1);
                self.ready[port] = true;
            }
            if !self.opened[port] {
                self.control_event(port as u32, PORT_OPEN, 1);
                self.opened[port] = true;
            }
        }
    }

    /// 发送一条控制消息。
    fn control_event(&mut self, port: u32, event: u16, value: u16) {
        let Some((_, transmit)) = self.control.as_mut() else {
            return;
        };
        let mut message = [0_u8; 8];
        message[0..4].copy_from_slice(&port.to_le_bytes());
        message[4..6].copy_from_slice(&event.to_le_bytes());
        message[6..8].copy_from_slice(&value.to_le_bytes());
        let _ = transmit.add_notify_wait_pop(&[&message], &mut [], &mut self.transport);
    }

    /// 把控制接收缓冲挂到控制队列。
    fn arm_control(&mut self) {
        if self.control_token.is_some() {
            return;
        }
        let Some((receive, _)) = self.control.as_mut() else {
            return;
        };
        let mut outputs: [&mut [u8]; 1] = [self.control_buffer.as_mut_slice()];
        // SAFETY: 控制缓冲在令牌回收前保持有效。
        if let Ok(token) = unsafe { receive.add(&[], &mut outputs) } {
            if receive.should_notify() {
                self.transport.notify(CONTROL_RX);
            }
            self.control_token = Some(token);
        }
    }

    /// 读走控制消息并按事件回应；服务运行期也会被调用，处理 RESIZE/REMOVE。
    fn harvest_control(&mut self) {
        let Some((receive, _)) = self.control.as_mut() else {
            return;
        };
        let Some(token) = self.control_token else {
            self.arm_control();
            return;
        };
        if !receive.can_pop() {
            return;
        }
        let mut outputs: [&mut [u8]; 1] = [self.control_buffer.as_mut_slice()];
        // SAFETY: 该令牌正是 `arm_control` 提交时的那组缓冲。
        let _ = unsafe { receive.pop_used(token, &[], &mut outputs) };
        self.control_token = None;
        let id = u32::from_le_bytes(self.control_buffer[0..4].try_into().unwrap_or([0; 4]));
        let event = u16::from_le_bytes(self.control_buffer[4..6].try_into().unwrap_or([0; 2]));
        if event == PORT_ADD
            && let Some(slot) = self.added.get_mut(id as usize)
        {
            *slot = true;
        }
        match event {
            PORT_ADD => {
                if !self.ready.get(id as usize).copied().unwrap_or(true) {
                    self.control_event(id, PORT_READY, 1);
                    if let Some(ready) = self.ready.get_mut(id as usize) {
                        *ready = true;
                    }
                }
            }
            CONSOLE_PORT | PORT_NAME if !self.opened.get(id as usize).copied().unwrap_or(true) => {
                self.control_event(id, PORT_OPEN, 1);
                if let Some(opened) = self.opened.get_mut(id as usize) {
                    *opened = true;
                }
            }
            _ => {}
        }
        self.arm_control();
    }

    /// 处理待到达的控制事件；服务可在每次请求前调用。
    pub fn poll_control(&mut self) {
        self.harvest_control();
    }

    /// 切回本设备的授权；同一进程里的多个设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// 端口数量。
    pub fn ports(&self) -> usize {
        self.ports.len()
    }

    /// 返回控制台字符尺寸（`SIZE` 特性）。
    pub fn size(&self) -> Option<(u16, u16)> {
        self.size
    }

    /// 向指定端口发送字节。
    pub fn send_on(&mut self, port: usize, bytes: &[u8]) -> Result<()> {
        self.activate();
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(state) = self.ports.get_mut(port) else {
            return Err(Error::InvalidParam);
        };
        state
            .transmit
            .add_notify_wait_pop(&[bytes], &mut [], &mut self.transport)
            .map(|_| ())
    }

    /// 从指定端口取一个字节；没有数据时返回 `None`。
    pub fn receive_on(&mut self, port: usize) -> Option<u8> {
        self.activate();
        let state = self.ports.get_mut(port)?;
        if state.cursor == state.length {
            state.length = 0;
            state.cursor = 0;
            if state.harvest(&mut self.transport) == 0 {
                return None;
            }
        }
        let byte = state.buffer[state.cursor];
        state.cursor += 1;
        if state.cursor == state.length {
            state.arm(&mut self.transport);
        }
        Some(byte)
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&mut self) -> bool {
        self.activate();
        self.transport.ack_interrupt().bits() != 0
    }

    /// 向 port 0 发送字节。
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.send_on(0, bytes)
    }

    /// 从 port 0 取一个字节。
    pub fn receive(&mut self) -> Option<u8> {
        self.receive_on(0)
    }
}
