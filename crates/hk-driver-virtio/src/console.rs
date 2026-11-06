//! virtio-console 设备封装。
//!
//! 只使用端口 0 的接收/发送队列；读取不阻塞，没有数据时返回 `None`，
//! 服务在自己的主循环里按请求轮询。

use hk_hal::DeviceGrant;
use spin::Mutex;
use virtio_drivers::device::console::VirtIOConsole;
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::{self, HalImpl};

/// virtio 控制台设备。
pub struct Console {
    console: Mutex<VirtIOConsole<HalImpl, PciTransport>>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Console {
    pub(crate) fn new(console: VirtIOConsole<HalImpl, PciTransport>) -> Self {
        Self {
            console: Mutex::new(console),
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
        }
    }

    /// 切回本设备的授权；同一进程里的多个设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// 返回控制台字符尺寸；设备未声明 `SIZE` 特性时为 `None`。
    pub fn size(&self) -> Option<(u16, u16)> {
        self.activate();
        self.console
            .lock()
            .size()
            .ok()
            .flatten()
            .map(|size| (size.columns, size.rows))
    }

    /// 发送一段字节。
    pub fn send(&self, bytes: &[u8]) -> Result<(), virtio_drivers::Error> {
        self.activate();
        self.console.lock().send_bytes(bytes)
    }

    /// 取下一个已到达的字节；没有数据时返回 `None`。
    pub fn receive(&self) -> Option<u8> {
        self.activate();
        self.console.lock().recv(true).ok().flatten()
    }

    /// 确认设备中断并完成已就绪的接收请求。
    pub fn ack_interrupt(&self) -> bool {
        self.activate();
        self.console.lock().ack_interrupt().unwrap_or(false)
    }
}
