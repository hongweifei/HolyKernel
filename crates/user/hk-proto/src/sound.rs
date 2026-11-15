//! virtio-sound 服务协议与客户端。
//!
//! 属于 virtio-sound 驱动层：`SOUND_INFO` 返回输出流数量与自检音参数，
//! `SOUND_TONE` 让驱动重放一段 440 Hz 自检音。

use crate::labels::VIRTIO;
use hk_server_api::{recv_data_try, send, yield_now};

/// 查询音频能力。
pub const SOUND_INFO: u64 = VIRTIO + 0x50;
/// 能力响应；载荷为 `streams`、`rate`、`channels`、`jacks`（各 4 字节小端）。
pub const SOUND_INFO_REPLY: u64 = VIRTIO + 0x51;
/// 播放正弦音；消息字为频率（Hz），0 表示自检频率 440。
pub const SOUND_TONE: u64 = VIRTIO + 0x52;
/// 播放结果；消息字为 1 表示成功。
pub const SOUND_TONE_ACK: u64 = VIRTIO + 0x53;
/// 握手请求。
pub const SOUND_PING: u64 = VIRTIO + 0x54;
/// 握手响应。
pub const SOUND_PONG: u64 = VIRTIO + 0x55;
/// 请求驱动服务退出。
pub const SOUND_SHUTDOWN: u64 = VIRTIO + 0x56;

/// 提供音频设备的服务名。
pub const SOUND_SERVICE: &str = "hk-server-virtio";

/// 等待响应的最大轮次。
const WAIT_ATTEMPTS: usize = 20_000;

/// 音频客户端。
#[derive(Clone, Copy)]
pub struct Client {
    endpoint: u64,
    service: u64,
}

impl Client {
    /// 等待 virtio 驱动服务并构造客户端。
    pub fn discover(endpoint: u64) -> Option<Self> {
        let service = hk_server_api::runtime::wait_service(SOUND_SERVICE)?;
        Some(Self { endpoint, service })
    }

    /// 查询输出流数量、采样率、声道数与 jack 数。
    pub fn info(&self) -> Option<(u32, u32, u32, u32)> {
        let _ = send(self.service, SOUND_INFO, 0);
        let mut buffer = [0_u8; 16];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, word, sender, length) = recv_data_try(self.endpoint, &mut buffer);
            if label == SOUND_INFO_REPLY && sender == self.service && length >= 16 {
                let _ = word;
                let streams = u32::from_le_bytes(buffer[0..4].try_into().ok()?);
                let rate = u32::from_le_bytes(buffer[4..8].try_into().ok()?);
                let channels = u32::from_le_bytes(buffer[8..12].try_into().ok()?);
                let jacks = u32::from_le_bytes(buffer[12..16].try_into().ok()?);
                return Some((streams, rate, channels, jacks));
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        None
    }

    /// 播放自检音（440 Hz）。
    pub fn tone(&self) -> bool {
        self.tone_at(0)
    }

    /// 播放指定频率的正弦音；`hz = 0` 表示自检频率。
    pub fn tone_at(&self, hz: u32) -> bool {
        let _ = send(self.service, SOUND_TONE, u64::from(hz));
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == SOUND_TONE_ACK && sender == self.service {
                return word == 1;
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        false
    }

    /// 握手；服务就绪时返回 `true`。
    pub fn ping(&self) -> bool {
        let _ = send(self.service, SOUND_PING, 0);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == SOUND_PONG && sender == self.service {
                return true;
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        false
    }
}
