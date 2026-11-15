//! virtio-gpu 扫描输出处理。

use hk_driver_virtio::Gpu;
use hk_driver_virtio::protocol::scanout::{
    SCANOUT_FLUSH, SCANOUT_FLUSH_ACK, SCANOUT_INFO, SCANOUT_INFO_REPLY, SCANOUT_PATTERN,
    SCANOUT_PATTERN_ACK, SCANOUT_PING, SCANOUT_PONG, SCANOUT_SHUTDOWN,
};
use hk_server_api::{exit, send, send_data};

/// 处理扫描输出协议消息。
pub fn handle(gpu: &Gpu, label: u64, sender: u64) {
    match label {
        SCANOUT_INFO => {
            let mut payload = [0_u8; 8];
            let known = gpu
                .resolution()
                .map(|(width, height)| {
                    payload[0..4].copy_from_slice(&width.to_le_bytes());
                    payload[4..8].copy_from_slice(&height.to_le_bytes());
                    true
                })
                .unwrap_or(false);
            if known {
                let _ = send_data(sender, SCANOUT_INFO_REPLY, &payload);
            }
        }
        SCANOUT_PATTERN => {
            let ok = gpu.pattern().is_some();
            let _ = send(sender, SCANOUT_PATTERN_ACK, u64::from(ok));
        }
        SCANOUT_FLUSH => {
            let _ = send(sender, SCANOUT_FLUSH_ACK, u64::from(gpu.flush()));
        }
        SCANOUT_PING => {
            let _ = send(sender, SCANOUT_PONG, 0);
        }
        SCANOUT_SHUTDOWN => exit(0),
        _ => {}
    }
}
