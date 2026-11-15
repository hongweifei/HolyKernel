//! 通用设备授权协议：驱动按设备类申请授权。

pub use hk_syscall::{DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};

/// 任意 virtio 设备。
pub const DEVICE_CLASS_ANY: u64 = 0;
/// 块设备。
pub const DEVICE_CLASS_BLOCK: u64 = 1;
/// 网络设备。
pub const DEVICE_CLASS_NETWORK: u64 = 2;
/// 输入设备。
pub const DEVICE_CLASS_INPUT: u64 = 3;
/// 熵源设备。
pub const DEVICE_CLASS_RNG: u64 = 4;
/// 控制台设备。
pub const DEVICE_CLASS_CONSOLE: u64 = 5;
/// AHCI/SATA 控制器。
pub const DEVICE_CLASS_AHCI: u64 = 6;
/// NVMe 控制器。
pub const DEVICE_CLASS_NVME: u64 = 7;
/// USB 主机控制器。
pub const DEVICE_CLASS_USB: u64 = 8;
/// 扫描输出（virtio-gpu）。
pub const DEVICE_CLASS_GPU: u64 = 9;
/// 音频播放（virtio-sound）。
pub const DEVICE_CLASS_SOUND: u64 = 10;
