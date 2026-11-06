//! virtio-console 服务协议与客户端。
//!
//! 属于 virtio-console 驱动层，不进入内核提供给所有服务的通用 API。客户端发送
//! `VCONSOLE_WRITE` 写入字节，服务回送 `VCONSOLE_ACK`；发送 `VCONSOLE_READ`
//! 取回已到达的输入字节，没有数据时响应载荷为空。

use hk_server_api::{recv_data_try, send, send_data, yield_now};

/// 写入字节；载荷为数据。
pub const VCONSOLE_WRITE: u64 = 0xC10;
/// 读取输入字节。
pub const VCONSOLE_READ: u64 = 0xC11;
/// 读取响应；载荷为数据。
pub const VCONSOLE_DATA: u64 = 0xC12;
/// 写入完成；消息字为写入长度。
pub const VCONSOLE_ACK: u64 = 0xC13;
/// 握手请求。
pub const VCONSOLE_PING: u64 = 0xC14;
/// 握手响应。
pub const VCONSOLE_PONG: u64 = 0xC15;
/// 请求驱动服务退出。
pub const VCONSOLE_SHUTDOWN: u64 = 0xC16;

/// 单次读写的最大字节数。
pub const VCONSOLE_MAX: usize = 256;

/// 等待响应的最大轮次；服务尚未就绪时客户端返回 0 而不是永久阻塞。
const WAIT_ATTEMPTS: usize = 20_000;

/// 控制台客户端。
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

    /// 返回控制台服务端点。
    pub fn service_endpoint(&self) -> u64 {
        self.service
    }

    /// 写入字节，返回服务确认的长度。
    pub fn write(&self, bytes: &[u8]) -> usize {
        let bytes = &bytes[..bytes.len().min(VCONSOLE_MAX)];
        let _ = send_data(self.service, VCONSOLE_WRITE, bytes);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == VCONSOLE_ACK && sender == self.service {
                return (word as usize).min(bytes.len());
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        0
    }

    /// 读取已到达的输入字节，返回长度；没有数据时返回 0。
    pub fn read(&self, buffer: &mut [u8]) -> usize {
        if buffer.is_empty() {
            return 0;
        }
        let limit = buffer.len().min(VCONSOLE_MAX);
        let _ = send(self.service, VCONSOLE_READ, 0);
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, length) = recv_data_try(self.endpoint, buffer);
            if label == VCONSOLE_DATA && sender == self.service {
                return length.min(limit);
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        0
    }

    /// 握手；服务就绪时返回 `true`。
    pub fn ping(&self) -> bool {
        let _ = send(self.service, VCONSOLE_PING, 0);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == VCONSOLE_PONG && sender == self.service {
                return true;
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        false
    }
}
