//! virtio-gpu 设备封装。
//!
//! 只做 2D 扫描输出：按设备默认分辨率建立资源、把资源挂到扫描输出、绘制诊断
//! 图案并 `flush` 到宿主。驱动自己持有资源内存是 `virtio-drivers` 的内部
//! `Dma`，目前不暴露给其他进程；把扫描输出交给 Display 需要外置 backing
//! （见 docs/roadmap 的 virtio 待办）。

use hk_device::DeviceGrant;
use spin::Mutex;
use virtio_drivers::device::gpu::VirtIOGpu;
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::{self, HalImpl};

/// virtio-gpu 设备。
pub struct Gpu {
    gpu: Mutex<VirtIOGpu<HalImpl, PciTransport>>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Gpu {
    pub(crate) fn new(gpu: VirtIOGpu<HalImpl, PciTransport>) -> Self {
        Self {
            gpu: Mutex::new(gpu),
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
        }
    }

    /// 切回本设备的授权；同一进程里的多个设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// 返回设备声明的扫描输出分辨率。
    pub fn resolution(&self) -> Option<(u32, u32)> {
        self.activate();
        self.gpu.lock().resolution().ok()
    }

    /// 建立扫描输出并绘制诊断图案，返回分辨率；用于启动自检。
    pub fn pattern(&self) -> Option<(u32, u32)> {
        self.activate();
        let mut gpu = self.gpu.lock();
        let (width, height) = gpu.resolution().ok()?;
        let buffer = gpu.change_resolution(width, height).ok()?;
        paint(buffer, width, height);
        gpu.flush().ok()?;
        Some((width, height))
    }

    /// 把当前帧缓冲提交给宿主。
    pub fn flush(&self) -> bool {
        self.activate();
        self.gpu.lock().flush().is_ok()
    }

    /// 确认设备中断。
    pub fn ack_interrupt(&self) {
        self.activate();
        let _ = self.gpu.lock().ack_interrupt();
    }
}

/// 交叉色带 + 上下白边的诊断图案，按 XRGB8888（字节序 B,G,R,X）填充。
fn paint(buffer: &mut [u8], width: u32, height: u32) {
    const BARS: [u32; 8] = [
        0x00E0_4040,
        0x0040_E040,
        0x0040_40E0,
        0x00E0_E040,
        0x00E0_40E0,
        0x0040_E0E0,
        0x00FF_FFFF,
        0x0020_2020,
    ];
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 || buffer.len() < width * height * 4 {
        return;
    }
    for row in 0..height {
        let edge = row < 8 || row + 8 >= height;
        for column in 0..width {
            let color = if edge {
                0x00FF_FFFF
            } else {
                BARS[column * BARS.len() / width]
            };
            let index = (row * width + column) * 4;
            buffer[index] = (color & 0xFF) as u8;
            buffer[index + 1] = ((color >> 8) & 0xFF) as u8;
            buffer[index + 2] = ((color >> 16) & 0xFF) as u8;
            buffer[index + 3] = 0xFF;
        }
    }
}
