//! virtio-input 设备封装。

use hk_hal::DeviceGrant;
use virtio_drivers::device::input::{InputEvent, VirtIOInput};
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::{self, HalImpl};

/// virtio 输入设备。
pub struct Input {
    input: VirtIOInput<HalImpl, PciTransport>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Input {
    pub(crate) fn new(transport: PciTransport) -> Result<Self, virtio_drivers::Error> {
        let input = VirtIOInput::new(transport)?;
        Ok(Self {
            input,
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
        })
    }

    /// 切回本设备的授权；同一进程里的多个输入设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&mut self) {
        self.activate();
        let _ = self.input.ack_interrupt();
    }

    /// 取出一个待处理事件。
    pub fn pop_event(&mut self) -> Option<InputEvent> {
        self.activate();
        self.input.pop_pending_event()
    }
}
