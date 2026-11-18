//! virtio-gpu 扫描输出处理。

use hk_driver_virtio::Gpu;
use hk_proto::scanout::{
    SCANOUT_FLUSH, SCANOUT_FLUSH_ACK, SCANOUT_INFO, SCANOUT_INFO_REPLY, SCANOUT_PATTERN,
    SCANOUT_PATTERN_ACK, SCANOUT_PING, SCANOUT_PONG, SCANOUT_REGION, SCANOUT_REGION_INFO,
    SCANOUT_SHUTDOWN,
};
use hk_user_api::{exit, send, send_data, send_handle};

/// 交给 Display 的权利：读/写（映射并绘制）与转移（句柄要经 IPC 交付）。
const REGION_RIGHTS: u32 = 0x1 | 0x2 | 0x8;

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
        SCANOUT_REGION => {
            let Some((capability, width, height, stride)) = gpu.scanout() else {
                hk_user_api::write(b"hk-server-virtio: gpu scanout missing\n");
                return;
            };
            // 复制一份句柄交给调用方：驱动自己还要用原句柄关闭分配。
            let transferred = hk_user_api::handle_duplicate(capability, REGION_RIGHTS);
            if transferred == 0 || transferred == u64::MAX {
                return;
            }
            let mut payload = [0_u8; 12];
            payload[0..4].copy_from_slice(&width.to_le_bytes());
            payload[4..8].copy_from_slice(&height.to_le_bytes());
            payload[8..12].copy_from_slice(&stride.to_le_bytes());
            let _ = send_data(sender, SCANOUT_REGION_INFO, &payload);
            let _ = send_handle(sender, SCANOUT_REGION, 0, transferred);
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
