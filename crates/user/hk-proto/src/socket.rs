//! 原生 Socket 协议与客户端。
//!
//! 请求与响应都以字节载荷编码，句柄与状态放在首个 `u64`。客户端只依赖本
//! crate，供用户态服务使用；内核与 Shell 不感知该协议。

use hk_server_api::{recv_data_try, send_data, service_endpoint, ticks, yield_now};

/// 打开 socket；载荷为 [`SocketKind`]。
pub const SOCK_OPEN: u64 = crate::labels::SOCKET;
/// TCP 连接；载荷为句柄 + IPv4 + 端口。
pub const SOCK_CONNECT: u64 = crate::labels::SOCKET + 1;
/// TCP 发送；载荷为句柄 + 数据。
pub const SOCK_SEND: u64 = crate::labels::SOCKET + 2;
/// TCP 接收；载荷为句柄 + 最大长度。
pub const SOCK_RECV: u64 = crate::labels::SOCKET + 3;
/// 关闭 socket；载荷为句柄。
pub const SOCK_CLOSE: u64 = crate::labels::SOCKET + 4;
/// UDP 发送；载荷为句柄 + IPv4 + 端口 + 数据。
pub const SOCK_UDP_SEND: u64 = crate::labels::SOCKET + 5;
/// 解析名称；载荷为 UTF-8 名称。
pub const SOCK_RESOLVE: u64 = crate::labels::SOCKET + 6;
/// 统一响应标签；载荷首 8 字节为结果。
pub const SOCK_RESPONSE: u64 = crate::labels::SOCKET + 7;
/// UDP 接收；载荷为句柄 + 最大长度。
pub const SOCK_UDP_RECV: u64 = crate::labels::SOCKET + 8;

/// TCP socket。
pub const SOCK_KIND_TCP: u8 = 1;
/// UDP socket。
pub const SOCK_KIND_UDP: u8 = 2;

/// 句柄编码长度。
pub const SOCK_HANDLE_BYTES: usize = 8;
/// 地址编码长度：IPv4 + 端口。
pub const SOCK_ADDR_BYTES: usize = 6;
/// 单次收发载荷上限。
pub const SOCK_PAYLOAD_MAX: usize = 1514;
/// 错误结果。
pub const SOCK_ERROR: u64 = u64::MAX;

/// Socket 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketKind {
    /// 流式 TCP。
    Tcp,
    /// 数据报 UDP。
    Udp,
}

impl SocketKind {
    const fn code(self) -> u8 {
        match self {
            Self::Tcp => SOCK_KIND_TCP,
            Self::Udp => SOCK_KIND_UDP,
        }
    }
}

/// 用户态 Socket 客户端。
#[derive(Clone, Copy)]
pub struct Client {
    endpoint: u64,
    net: u64,
}

impl Client {
    /// 等待 Network Server 并构造客户端；失败返回 `None`。
    pub fn discover(endpoint: u64) -> Option<Self> {
        for _ in 0..4096 {
            let net = service_endpoint("hk-server-net");
            if net != 0 {
                return Some(Self { endpoint, net });
            }
            let _ = yield_now();
        }
        None
    }

    /// 打开一个 socket，返回句柄。
    pub fn open(&self, kind: SocketKind) -> Option<u64> {
        let request = [kind.code()];
        let mut response = [0_u8; 32];
        let (handle, _) = self.exchange(SOCK_OPEN, &request, &mut response)?;
        Some(handle)
    }

    /// TCP 连接。
    pub fn connect(&self, handle: u64, address: [u8; 4], port: u16) -> bool {
        let mut request = [0_u8; SOCK_HANDLE_BYTES + SOCK_ADDR_BYTES];
        request[..8].copy_from_slice(&handle.to_le_bytes());
        request[8..12].copy_from_slice(&address);
        request[12..14].copy_from_slice(&port.to_le_bytes());
        let mut response = [0_u8; 16];
        self.exchange(SOCK_CONNECT, &request, &mut response)
            .is_some()
    }

    /// 发送 TCP 数据，返回写入长度。
    pub fn send(&self, handle: u64, data: &[u8]) -> usize {
        let length = data.len().min(SOCK_PAYLOAD_MAX);
        let mut request = [0_u8; SOCK_HANDLE_BYTES + SOCK_PAYLOAD_MAX];
        request[..8].copy_from_slice(&handle.to_le_bytes());
        request[8..8 + length].copy_from_slice(&data[..length]);
        let mut response = [0_u8; 16];
        match self.exchange(SOCK_SEND, &request[..8 + length], &mut response) {
            Some((written, _)) => written as usize,
            None => 0,
        }
    }

    /// 接收 TCP 数据；返回实际读出的字节数（0 表示暂无数据）。
    pub fn recv(&self, handle: u64, out: &mut [u8]) -> usize {
        let mut request = [0_u8; SOCK_HANDLE_BYTES + 2];
        request[..8].copy_from_slice(&handle.to_le_bytes());
        request[8..10].copy_from_slice(&(out.len().min(SOCK_PAYLOAD_MAX) as u16).to_le_bytes());
        let mut response = [0_u8; SOCK_PAYLOAD_MAX + 8];
        match self.exchange(SOCK_RECV, &request, &mut response) {
            Some((length, payload)) => {
                let length = (length as usize).min(payload).min(out.len());
                out[..length].copy_from_slice(&response[8..8 + length]);
                length
            }
            None => 0,
        }
    }

    /// 发送 UDP 数据，返回写入长度。
    pub fn udp_send(&self, handle: u64, address: [u8; 4], port: u16, data: &[u8]) -> usize {
        let length = data.len().min(SOCK_PAYLOAD_MAX);
        let mut request = [0_u8; SOCK_HANDLE_BYTES + SOCK_ADDR_BYTES + SOCK_PAYLOAD_MAX];
        request[..8].copy_from_slice(&handle.to_le_bytes());
        request[8..12].copy_from_slice(&address);
        request[12..14].copy_from_slice(&port.to_le_bytes());
        request[14..14 + length].copy_from_slice(&data[..length]);
        let mut response = [0_u8; 16];
        match self.exchange(SOCK_UDP_SEND, &request[..14 + length], &mut response) {
            Some((written, _)) => written as usize,
            None => 0,
        }
    }

    /// 接收 UDP 数据；返回实际读出的字节数（0 表示暂无数据）。
    pub fn udp_recv(&self, handle: u64, out: &mut [u8]) -> usize {
        let mut request = [0_u8; SOCK_HANDLE_BYTES + 2];
        request[..8].copy_from_slice(&handle.to_le_bytes());
        request[8..10].copy_from_slice(&(out.len().min(SOCK_PAYLOAD_MAX) as u16).to_le_bytes());
        let mut response = [0_u8; SOCK_PAYLOAD_MAX + 8];
        match self.exchange(SOCK_UDP_RECV, &request, &mut response) {
            Some((length, payload)) => {
                let length = (length as usize).min(payload).min(out.len());
                out[..length].copy_from_slice(&response[8..8 + length]);
                length
            }
            None => 0,
        }
    }

    /// 关闭 socket。
    pub fn close(&self, handle: u64) {
        let mut request = [0_u8; 8];
        request.copy_from_slice(&handle.to_le_bytes());
        let mut response = [0_u8; 16];
        let _ = self.exchange(SOCK_CLOSE, &request, &mut response);
    }

    /// 解析名称，把 IPv4 地址写入 `out`，返回地址数量。
    pub fn resolve(&self, name: &str, out: &mut [u8]) -> usize {
        let bytes = name.as_bytes();
        let length = bytes.len().min(64);
        let mut response = [0_u8; 8 + 4 * 4];
        let Some((count, payload)) =
            self.exchange_with(SOCK_RESOLVE, &bytes[..length], &mut response, 1, 1500)
        else {
            return 0;
        };
        let count = (count as usize).min(payload / 4).min(out.len() / 4);
        out[..count * 4].copy_from_slice(&response[8..8 + count * 4]);
        count
    }

    fn exchange(&self, label: u64, request: &[u8], response: &mut [u8]) -> Option<(u64, usize)> {
        self.exchange_with(label, request, response, 50, 10)
    }

    /// 有界重试：Network Server 可能仍在 DHCP/自检，早发的请求会被丢弃。
    fn exchange_with(
        &self,
        label: u64,
        request: &[u8],
        response: &mut [u8],
        attempts: u32,
        timeout_ticks: u64,
    ) -> Option<(u64, usize)> {
        for _ in 0..attempts {
            if send_data(self.net, label, request) != 0 {
                return None;
            }
            let deadline = ticks().saturating_add(timeout_ticks);
            while ticks() < deadline {
                let (reply, _word, sender, length) = recv_data_try(self.endpoint, response);
                if reply == SOCK_RESPONSE && sender == self.net && length >= 8 {
                    let result = u64::from_le_bytes(response[..8].try_into().ok()?);
                    if result == SOCK_ERROR {
                        return None;
                    }
                    return Some((result, length - 8));
                }
                let _ = yield_now();
            }
        }
        None
    }
}
