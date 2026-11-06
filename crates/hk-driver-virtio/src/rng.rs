//! virtio-rng 设备封装。
//!
//! 单队列请求/响应：每次 [`Rng::fill`] 把目标缓冲提交给设备并等待完成。
//! 设备不产生需要清除的中断状态，服务可以不绑定 IRQ。

use hk_hal::DeviceGrant;
use spin::Mutex;
use virtio_drivers::device::rng::VirtIORng;
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::{self, HalImpl};

/// virtio 熵源设备。
pub struct Rng {
    rng: Mutex<VirtIORng<HalImpl, PciTransport>>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Rng {
    pub(crate) fn new(rng: VirtIORng<HalImpl, PciTransport>) -> Self {
        Self {
            rng: Mutex::new(rng),
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
        }
    }

    /// 切回本设备的授权；同一进程里的多个设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// 从设备读取随机字节，返回实际写入的长度。
    pub fn fill(&self, buffer: &mut [u8]) -> Result<usize, virtio_drivers::Error> {
        self.activate();
        self.rng.lock().request_entropy(buffer)
    }
}
