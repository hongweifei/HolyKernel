//! 熵源服务协议与客户端。
//!
//! 属于 virtio-rng 驱动层，不进入内核提供给所有服务的通用 API。请求字为期望
//! 长度，响应载荷为实际字节；单次请求上限 [`ENTROPY_MAX`]。

use hk_server_api::{recv_data_try, send, yield_now};

/// 请求随机字节；消息字为长度。
pub const ENTROPY_REQUEST: u64 = 0xC00;
/// 随机字节响应；载荷为字节。
pub const ENTROPY_RESPONSE: u64 = 0xC01;
/// 握手请求。
pub const ENTROPY_PING: u64 = 0xC02;
/// 握手响应。
pub const ENTROPY_PONG: u64 = 0xC03;
/// 请求驱动服务退出。
pub const ENTROPY_SHUTDOWN: u64 = 0xC04;

/// 单次请求的最大字节数。
pub const ENTROPY_MAX: usize = 256;

/// 等待响应的最大轮次；服务尚未就绪时客户端返回 0 而不是永久阻塞。
const WAIT_ATTEMPTS: usize = 20_000;

/// 熵源客户端。
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

    /// 返回熵源服务端点。
    pub fn service_endpoint(&self) -> u64 {
        self.service
    }

    /// 读取随机字节，返回实际写入的长度。
    pub fn fill(&self, buffer: &mut [u8]) -> usize {
        if buffer.is_empty() {
            return 0;
        }
        let wanted = buffer.len().min(ENTROPY_MAX);
        let _ = send(self.service, ENTROPY_REQUEST, wanted as u64);
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, length) = recv_data_try(self.endpoint, buffer);
            if label == ENTROPY_RESPONSE && sender == self.service {
                return length.min(wanted);
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        0
    }

    /// 握手；服务就绪时返回 `true`。
    pub fn ping(&self) -> bool {
        let _ = send(self.service, ENTROPY_PING, 0);
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, _word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == ENTROPY_PONG && sender == self.service {
                return true;
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        false
    }
}
