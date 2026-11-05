//! virtio-input 设备封装。

use virtio_drivers::device::input::{InputEvent, VirtIOInput};
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::HalImpl;

/// virtio 输入设备。
pub struct Input {
    input: VirtIOInput<HalImpl, PciTransport>,
}

impl Input {
    pub(crate) fn new(transport: PciTransport) -> Result<Self, virtio_drivers::Error> {
        Ok(Self {
            input: VirtIOInput::new(transport)?,
        })
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&mut self) {
        let _ = self.input.ack_interrupt();
    }

    /// 取出一个待处理事件。
    pub fn pop_event(&mut self) -> Option<InputEvent> {
        self.input.pop_pending_event()
    }
}
