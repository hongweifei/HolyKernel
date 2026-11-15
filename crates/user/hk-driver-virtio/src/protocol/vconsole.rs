//! virtio-console 服务协议与客户端。
//!
//! 属于 virtio-console 驱动层，不进入内核提供给所有服务的通用 API。客户端发送
//! `VCONSOLE_WRITE` 写入字节，服务回送 `VCONSOLE_ACK`；发送 `VCONSOLE_READ`
//! 取回已到达的输入字节，没有数据时响应载荷为空。

use hk_proto::labels::VIRTIO;
use hk_server_api::{recv_data_try, send, send_data, yield_now};

/// 写入字节；载荷前 4 字节为端口号（小端），其后为数据。
pub const VCONSOLE_WRITE: u64 = VIRTIO + 0x10;
/// 读取输入字节；消息字为端口号。
pub const VCONSOLE_READ: u64 = VIRTIO + 0x11;
/// 读取响应；载荷为数据，消息字为端口号。
pub const VCONSOLE_DATA: u64 = VIRTIO + 0x12;
/// 写入完成；消息字高 32 位为端口号，低 32 位为写入长度。
pub const VCONSOLE_ACK: u64 = VIRTIO + 0x13;
/// 握手请求。
pub const VCONSOLE_PING: u64 = VIRTIO + 0x14;
/// 握手响应。
pub const VCONSOLE_PONG: u64 = VIRTIO + 0x15;
/// 请求驱动服务退出。
pub const VCONSOLE_SHUTDOWN: u64 = VIRTIO + 0x16;

/// 单次读写的最大字节数。
pub const VCONSOLE_MAX: usize = 256;

/// 写入载荷中的端口头长度。
pub const VCONSOLE_PORT_HEADER: usize = 4;

/// 单次写入的最大数据字节数。
pub const VCONSOLE_MAX_WRITE: usize = VCONSOLE_MAX - VCONSOLE_PORT_HEADER;

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

    /// 写入 port 0，返回服务确认的长度。
    pub fn write(&self, bytes: &[u8]) -> usize {
        self.write_on(0, bytes)
    }

    /// 向指定端口写入字节，返回服务确认的长度。
    pub fn write_on(&self, port: u32, bytes: &[u8]) -> usize {
        let bytes = &bytes[..bytes.len().min(VCONSOLE_MAX_WRITE)];
        let mut payload = [0_u8; VCONSOLE_MAX];
        payload[..VCONSOLE_PORT_HEADER].copy_from_slice(&port.to_le_bytes());
        payload[VCONSOLE_PORT_HEADER..VCONSOLE_PORT_HEADER + bytes.len()].copy_from_slice(bytes);
        let _ = send_data(
            self.service,
            VCONSOLE_WRITE,
            &payload[..VCONSOLE_PORT_HEADER + bytes.len()],
        );
        let mut buffer = [0_u8; 8];
        for _ in 0..WAIT_ATTEMPTS {
            let (label, word, sender, _length) = recv_data_try(self.endpoint, &mut buffer);
            if label == VCONSOLE_ACK && sender == self.service {
                return ((word & 0xFFFF_FFFF) as usize).min(bytes.len());
            }
            if label == 0 {
                let _ = yield_now();
            }
        }
        0
    }

    /// 读取 port 0 的输入字节，返回长度；没有数据时返回 0。
    pub fn read(&self, buffer: &mut [u8]) -> usize {
        self.read_on(0, buffer)
    }

    /// 读取指定端口的输入字节，返回长度；没有数据时返回 0。
    pub fn read_on(&self, port: u32, buffer: &mut [u8]) -> usize {
        if buffer.is_empty() {
            return 0;
        }
        let limit = buffer.len().min(VCONSOLE_MAX);
        let _ = send(self.service, VCONSOLE_READ, u64::from(port));
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
