//! 通用设备授权协议：驱动按设备类申请授权。

pub use hk_syscall::{DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};

use crate::{close_handle, recv_handle, send};

/// 向设备管理器申请一类设备授权。
///
/// 管理器回复 `DEVICE_GRANT` 时返回授权句柄，回复 `DEVICE_NONE` 时返回
/// `None`；等待期间收到的其他句柄会被关闭。
pub fn request_grant(endpoint: u64, manager: u64, class: u64) -> Option<u64> {
    let _ = send(manager, DEVICE_REQUEST, class);
    loop {
        let (label, _word, _sender, handle) = recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return Some(handle);
        }
        if label == DEVICE_NONE {
            return None;
        }
        if handle != 0 {
            let _ = close_handle(handle);
        }
    }
}

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
