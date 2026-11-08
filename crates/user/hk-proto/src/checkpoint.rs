//! Checkpoint Server 协议：保存、恢复与列出 Native Task 检查点。

use hk_server_api::{recv_data_from, send_data};

/// 服务名。
pub const CHECKPOINT_SERVICE: &str = "hk-server-checkpoint";
/// 保存检查点；载荷为任务名。
pub const CHECKPOINT_SAVE: u64 = crate::labels::CHECKPOINT;
/// 恢复检查点；载荷为任务名。
pub const CHECKPOINT_RESTORE: u64 = crate::labels::CHECKPOINT + 1;
/// 列出检查点；无载荷。
pub const CHECKPOINT_LIST: u64 = crate::labels::CHECKPOINT + 2;
/// 操作结果；载荷为 UTF-8 文本。
pub const CHECKPOINT_RESULT: u64 = crate::labels::CHECKPOINT + 3;

/// 向服务发送请求并等待文本结果。
fn request(service: u64, own: u64, label: u64, name: &str, out: &mut [u8]) -> Result<usize, u64> {
    if send_data(service, label, name.as_bytes()) == u64::MAX {
        return Err(1);
    }
    let (reply, _word, sender, length) = recv_data_from(own, service, out);
    if reply != CHECKPOINT_RESULT || sender != service {
        return Err(2);
    }
    Ok(length.min(out.len()))
}

/// 保存任务检查点。
pub fn save(service: u64, own: u64, name: &str, out: &mut [u8]) -> Result<usize, u64> {
    request(service, own, CHECKPOINT_SAVE, name, out)
}

/// 恢复任务检查点。
pub fn restore(service: u64, own: u64, name: &str, out: &mut [u8]) -> Result<usize, u64> {
    request(service, own, CHECKPOINT_RESTORE, name, out)
}

/// 列出检查点。
pub fn list(service: u64, own: u64, out: &mut [u8]) -> Result<usize, u64> {
    request(service, own, CHECKPOINT_LIST, "", out)
}
