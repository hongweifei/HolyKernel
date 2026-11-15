//! 同一 virtio 驱动服务中的非块设备。

pub mod console;
pub mod gpu;
pub mod rng;
pub mod sound;

use hk_driver_virtio::{Console, Gpu, Rng, Sound};

/// 非块 virtio 设备集合。
#[derive(Default)]
pub struct Aux {
    /// 熵源设备。
    pub rng: Option<Rng>,
    /// 控制台设备。
    pub console: Option<Console>,
    /// 扫描输出设备。
    pub gpu: Option<Gpu>,
    /// 音频设备。
    pub sound: Option<Sound>,
}

impl Aux {
    /// 按协议标签分派一条消息；不属于自己的标签由各设备自行忽略。
    pub fn handle(&mut self, label: u64, word: u64, sender: u64, data: &[u8]) {
        if let Some(device) = &self.rng {
            rng::handle(device, label, word, sender);
        }
        if let Some(device) = &mut self.console {
            console::handle(device, label, word, sender, data);
        }
        if let Some(device) = &self.gpu {
            gpu::handle(device, label, sender);
        }
        if let Some(device) = &self.sound {
            sound::handle(device, label, sender);
        }
    }
}
