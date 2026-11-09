//! Tempo 策略客户端。
//!
//! 把目标 Cadence 快照发给 Tempo，接收随回复转移的子契约句柄。

use hk_proto::tempo::{
    MAX_NAME, SNAPSHOT_LEN, TEMPO_ADMIT, TEMPO_READMIT, TEMPO_REQUEST, TEMPO_RESPONSE, TEMPO_REVOKE,
};
use hk_server_api::{recv_data, recv_data_handle, send_data};

/// 回复缓冲上限。
const MAX_REPLY: usize = 128;
/// 按名请求缓冲上限。
const MAX_REQUEST: usize = 2 + MAX_NAME + SNAPSHOT_LEN;

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

    /// 按任务名读取当前契约并重准入。
    pub fn readmit_named(&mut self, name: &str) -> bool {
        if name.is_empty() || name.len() > MAX_NAME {
            return false;
        }
        let mut payload = [0_u8; 2 + MAX_NAME];
        payload[..2].copy_from_slice(&(name.len() as u16).to_le_bytes());
        payload[2..2 + name.len()].copy_from_slice(name.as_bytes());
        if send_data(self.remote, TEMPO_READMIT, &payload[..2 + name.len()]) != 0 {
            return false;
        }
        let (reply, _word, _sender, _size) = recv_data(self.local, &mut self.buffer);
        reply == TEMPO_RESPONSE
    }

    /// 按任务名准入或撤销重授予；成功返回 `true`。
    pub fn admit_named(&mut self, name: &str, snapshot: &[u8], regrant: bool) -> bool {
        if name.is_empty() || name.len() > MAX_NAME || snapshot.len() < SNAPSHOT_LEN {
            return false;
        }
        let mut payload = [0_u8; MAX_REQUEST];
        payload[..2].copy_from_slice(&(name.len() as u16).to_le_bytes());
        payload[2..2 + name.len()].copy_from_slice(name.as_bytes());
        let start = 2 + name.len();
        payload[start..start + SNAPSHOT_LEN].copy_from_slice(&snapshot[..SNAPSHOT_LEN]);
        let label = if regrant { TEMPO_REVOKE } else { TEMPO_ADMIT };
        if send_data(self.remote, label, &payload[..start + SNAPSHOT_LEN]) != 0 {
            return false;
        }
        let (reply, _word, _sender, _size) = recv_data(self.local, &mut self.buffer);
        reply == TEMPO_RESPONSE
    }
}
