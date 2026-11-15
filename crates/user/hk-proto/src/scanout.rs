//! virtio-gpu 扫描输出协议与客户端。
//!
//! `SCANOUT_INFO` 返回分辨率，`SCANOUT_REGION` 把驱动持有的扫描输出区域
//! （能力句柄）连同几何信息交给调用者，调用者映射后直接写入像素；每帧结束后
//! 用 `SCANOUT_FLUSH` 让驱动把内容提交给宿主。`SCANOUT_PATTERN` 是驱动自检。
//!
//! 协议留在 `hk-proto` 而不是驱动 crate，是因为 Display 是它的消费者。

use crate::labels::VIRTIO;
use hk_server_api::{close_handle, recv_data_handle_try, recv_data_try, send, yield_now};

/// 提供扫描输出的服务名（virtio 驱动服务）。
pub const SCANOUT_SERVICE: &str = "hk-server-virtio";

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
/// 索取扫描输出区域；几何信息与句柄分两条消息发出。
pub const SCANOUT_REGION: u64 = VIRTIO + 0x39;
/// 区域几何；载荷为 `width`、`height`、`stride`（各 4 字节小端，stride 按像素）。
pub const SCANOUT_REGION_INFO: u64 = VIRTIO + 0x3A;

/// 等待响应的最大轮次；服务尚未就绪时客户端返回失败而不是永久阻塞。
const WAIT_ATTEMPTS: usize = 20_000;

/// 扫描输出区域：能力句柄与几何信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// 可映射的能力句柄；调用方负责映射与关闭。
    pub capability: u64,
    /// 像素宽度。
    pub width: u32,
    /// 像素高度。
    pub height: u32,
    /// 每行像素数。
    pub stride: u32,
}

/// 扫描输出客户端。
#[derive(Clone, Copy)]
pub struct Client {
    endpoint: u64,
    service: u64,
}

impl Client {
    /// 等待 virtio 驱动服务并构造客户端。
    pub fn discover(endpoint: u64) -> Option<Self> {
        let service = hk_server_api::runtime::wait_service(SCANOUT_SERVICE)?;
        Some(Self::at(endpoint, service))
    }

    /// 以已知服务端点构造客户端。
    pub const fn at(endpoint: u64, service: u64) -> Self {
        Self { endpoint, service }
    }

    /// 返回服务端点。
    pub const fn service(&self) -> u64 {
        self.service
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

    /// 索取扫描输出区域的能力句柄与几何信息。
    pub fn region(&self) -> Option<Region> {
        let _ = send(self.service, SCANOUT_REGION, 0);
        let mut geometry = None;
        let mut capability = 0;
        let mut buffer = [0_u8; 12];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, length, handle) =
                recv_data_handle_try(self.endpoint, &mut buffer, 0);
            if label == 0 {
                let _ = yield_now();
                continue;
            }
            if sender != self.service {
                if handle != 0 {
                    let _ = close_handle(handle);
                }
                continue;
            }
            match label {
                SCANOUT_REGION_INFO if length >= 12 => {
                    geometry = Some((
                        u32::from_le_bytes(buffer[0..4].try_into().ok()?),
                        u32::from_le_bytes(buffer[4..8].try_into().ok()?),
                        u32::from_le_bytes(buffer[8..12].try_into().ok()?),
                    ));
                }
                SCANOUT_REGION if handle != 0 => capability = handle,
                _ => {}
            }
            if capability != 0
                && let Some((width, height, stride)) = geometry
            {
                return Some(Region {
                    capability,
                    width,
                    height,
                    stride,
                });
            }
        }
        if capability != 0 {
            let _ = close_handle(capability);
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
