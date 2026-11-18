//! virtio-sound 服务协议与客户端。
//!
//! 属于 virtio-sound 驱动层：`SOUND_INFO` 返回输出流数量与自检音参数，
//! `SOUND_TONE` 让驱动重放一段 440 Hz 自检音。

use crate::labels::VIRTIO;
use hk_user_api::{recv_data_try, send, yield_now};

/// 查询音频能力。
pub const SOUND_INFO: u64 = VIRTIO + 0x50;
/// 能力响应；载荷为 `streams`、`rate`、`channels`、`jacks`（各 4 字节小端）。
pub const SOUND_INFO_REPLY: u64 = VIRTIO + 0x51;
/// 播放正弦音；消息字为频率（Hz），0 表示自检频率 440。
pub const SOUND_TONE: u64 = VIRTIO + 0x52;
/// 请求受理；消息字为 1 表示已开始播放。
pub const SOUND_TONE_ACK: u64 = VIRTIO + 0x53;
/// 握手请求。
pub const SOUND_PING: u64 = VIRTIO + 0x54;
/// 握手响应。
pub const SOUND_PONG: u64 = VIRTIO + 0x55;
/// 播放结束；消息字为 1 表示完整播完，0 表示中止或失败。
pub const SOUND_DONE: u64 = VIRTIO + 0x57;
/// 查询播放状态；响应消息字 1=播放中、0=空闲/已播完、2=失败。
pub const SOUND_STATUS: u64 = VIRTIO + 0x58;
/// 状态响应：消息字为正在播放的流数量。
pub const SOUND_STATUS_REPLY: u64 = VIRTIO + 0x59;
/// 请求驱动服务退出。
pub const SOUND_SHUTDOWN: u64 = VIRTIO + 0x56;

/// 提供音频设备的服务名。
pub const SOUND_SERVICE: &str = "hk-server-virtio-sound";

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
        let service = hk_user_api::runtime::wait_service(SOUND_SERVICE)?;
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

    /// 开始一段指定频率与时长的播放，返回是否被受理（不等待播完）。
    pub fn play(&self, hz: u32, millis: u32) -> bool {
        let mut payload = [0_u8; 8];
        payload[0..4].copy_from_slice(&hz.to_le_bytes());
        payload[4..8].copy_from_slice(&millis.to_le_bytes());
        let _ = hk_user_api::send_data(self.service, SOUND_TONE, &payload);
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

    /// 查询正在播放的流数量。
    pub fn status(&self) -> u32 {
        let _ = send(self.service, SOUND_STATUS, 0);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == SOUND_STATUS_REPLY && sender == self.service {
                return word as u32;
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        2
    }

    /// 阻塞等待播放结束通知；返回是否完整播完。
    ///
    /// 播放由音频服务在定时节拍上推进，客户端不需要轮询。
    pub fn wait_done(&self) -> bool {
        loop {
            let (label, word, sender) = hk_user_api::recv(self.endpoint);
            if label == SOUND_DONE && sender == self.service {
                return word == 1;
            }
        }
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
