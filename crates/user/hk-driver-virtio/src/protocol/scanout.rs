//! virtio-gpu 扫描输出协议与客户端。
//!
//! 属于 virtio-gpu 驱动层：`SCANOUT_INFO` 返回分辨率，`SCANOUT_PATTERN` 让驱动
//! 重绘诊断图案并提交，`SCANOUT_FLUSH` 只提交当前帧。Display 接管扫描输出前
//! 用 `SCANOUT_INFO` 探测可用分辨率。

use hk_proto::labels::VIRTIO;
use hk_server_api::{recv_data_try, send, yield_now};

/// 查询扫描输出分辨率。
pub const SCANOUT_INFO: u64 = VIRTIO + 0x30;
/// 分辨率响应；载荷为 `width`、`height`（各 4 字节小端）。
pub const SCANOUT_INFO_REPLY: u64 = VIRTIO + 0x31;
/// 重绘诊断图案并提交。
pub const SCANOUT_PATTERN: u64 = VIRTIO + 0x32;
/// 图案绘制结果；消息字为 1 表示成功。
pub const SCANOUT_PATTERN_ACK: u64 = VIRTIO + 0x33;
/// 提交当前帧。
pub const SCANOUT_FLUSH: u64 = VIRTIO + 0x34;
/// 提交结果；消息字为 1 表示成功。
pub const SCANOUT_FLUSH_ACK: u64 = VIRTIO + 0x35;
/// 握手请求。
pub const SCANOUT_PING: u64 = VIRTIO + 0x36;
/// 握手响应。
pub const SCANOUT_PONG: u64 = VIRTIO + 0x37;
/// 请求驱动服务退出。
pub const SCANOUT_SHUTDOWN: u64 = VIRTIO + 0x38;

/// 等待响应的最大轮次；服务尚未就绪时客户端返回失败而不是永久阻塞。
const WAIT_ATTEMPTS: usize = 20_000;

/// 扫描输出客户端。
#[derive(Clone, Copy)]
pub struct Client {
    endpoint: u64,
    service: u64,
}

impl Client {
    /// 等待 virtio 驱动服务并构造客户端。
    pub fn discover(endpoint: u64) -> Option<Self> {
        let service = hk_server_api::runtime::wait_service(crate::SERVICE_NAME)?;
        Some(Self { endpoint, service })
    }

    /// 查询分辨率；设备没有扫描输出时返回 `None`。
    pub fn info(&self) -> Option<(u32, u32)> {
        let _ = send(self.service, SCANOUT_INFO, 0);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, length) = recv_data_try(self.endpoint, &mut buffer);
            if label == SCANOUT_INFO_REPLY && sender == self.service && length >= 8 {
                let width = u32::from_le_bytes(buffer[0..4].try_into().ok()?);
                let height = u32::from_le_bytes(buffer[4..8].try_into().ok()?);
                return Some((width, height));
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        None
    }

    /// 让驱动重绘诊断图案并提交。
    pub fn pattern(&self) -> bool {
        self.request(SCANOUT_PATTERN, SCANOUT_PATTERN_ACK)
    }

    /// 提交当前帧。
    pub fn flush(&self) -> bool {
        self.request(SCANOUT_FLUSH, SCANOUT_FLUSH_ACK)
    }

    /// 握手；服务就绪时返回 `true`。
    pub fn ping(&self) -> bool {
        self.request(SCANOUT_PING, SCANOUT_PONG)
    }

    fn request(&self, request: u64, expected: u64) -> bool {
        let _ = send(self.service, request, 0);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == expected && sender == self.service {
                return word == 1 || expected == SCANOUT_PONG;
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        false
    }
}
