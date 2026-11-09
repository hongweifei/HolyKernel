//! Tempo 策略客户端。
//!
//! 把目标 Cadence 快照发给 Tempo，接收随回复转移的子契约句柄。

use hk_proto::tempo::{TEMPO_REQUEST, TEMPO_RESPONSE};
use hk_server_api::{recv_data_handle, send_data};

/// 请求缓冲上限。
const MAX_REPLY: usize = 128;

/// Tempo 客户端。
pub struct TempoClient {
    remote: u64,
    local: u64,
    buffer: [u8; MAX_REPLY],
}

impl TempoClient {
    /// 以 Tempo 服务端点与本任务端点创建客户端。
    pub const fn new(remote: u64, local: u64) -> Self {
        Self {
            remote,
            local,
            buffer: [0; MAX_REPLY],
        }
    }

    /// 请求准入；成功返回子契约句柄编号。
    pub fn admit(&mut self, request: &[u8]) -> Option<u64> {
        if send_data(self.remote, TEMPO_REQUEST, request) != 0 {
            return None;
        }
        let (label, _word, _sender, _size, handle) =
            recv_data_handle(self.local, &mut self.buffer, 0);
        (label == TEMPO_RESPONSE && handle != 0).then_some(handle)
    }
}
