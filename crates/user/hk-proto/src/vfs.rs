//! VFS 客户端/服务端共享协议与轻量查询。

use crate::wire::{self, RequestHeader};
use hk_server_api::{recv_data_from, send_data, service_endpoint};

/// 请求标签。
pub const VFS_REQUEST: u64 = crate::labels::VFS;
/// 响应标签。
pub const VFS_RESPONSE: u64 = crate::labels::VFS + 1;
/// 附加共享 Frame。
pub const VFS_ATTACH: u64 = crate::labels::VFS + 2;
/// 解除共享 Frame。
pub const VFS_DETACH: u64 = crate::labels::VFS + 3;

/// 打开文件。
pub const OP_OPEN: u8 = 1;
/// 关闭文件。
pub const OP_CLOSE: u8 = 2;
/// 读取文件。
pub const OP_READ: u8 = 3;
/// 写入文件。
pub const OP_WRITE: u8 = 4;
/// 查询元数据。
pub const OP_STAT: u8 = 5;
/// 调整文件大小。
pub const OP_TRUNCATE: u8 = 6;
/// 删除文件或空目录。
pub const OP_UNLINK: u8 = 7;
/// 列出目录。
pub const OP_LIST: u8 = 8;
/// 创建目录。
pub const OP_MKDIR: u8 = 9;
/// 列出挂载点。
pub const OP_MOUNTS: u8 = 11;
/// 卸载挂载点。
pub const OP_UMOUNT: u8 = 12;
/// 刷写挂起写入。
pub const OP_SYNC: u8 = 13;
/// 从共享帧读取文件内容。
pub const OP_READ_BUFFER: u8 = 14;
/// 写入共享帧中的文件内容。
pub const OP_WRITE_BUFFER: u8 = 15;

/// 客户端共享帧长度。
pub const VFS_BUFFER_LEN: usize = 60 * 1024;

/// 打开读权限。
pub const OPEN_READ: u64 = 1 << 0;
/// 打开写权限。
pub const OPEN_WRITE: u64 = 1 << 1;
/// 不存在时创建。
pub const OPEN_CREATE: u64 = 1 << 2;

pub use crate::status::{
    Response, STATUS_DAMAGED, STATUS_ERROR, STATUS_EXISTS, STATUS_INVALID, STATUS_IO,
    STATUS_NOT_FOUND, STATUS_OK, STATUS_PERMISSION, STATUS_READ_ONLY, STATUS_STALE,
    decode_response, encode_response,
};

/// 请求头长度；布局由 公共帧头 统一。
const REQUEST_HEADER: usize = wire::REQUEST_HEADER;

/// 单条 VFS 协议消息的缓冲上限。
pub const VFS_MESSAGE_LEN: usize = 64 * 1024;

/// 查询 VFS 挂载表，把挂载点逐行写入 `out`。
///
/// 这是轻量只读查询，不建立共享 Frame；返回完整数据长度，调用方可用
/// `min(length, out.len())` 判断是否截断。
pub fn list_mounts(own_endpoint: u64, out: &mut [u8]) -> Result<usize, u8> {
    let endpoint = service_endpoint("hk-server-path");
    if endpoint == 0 {
        return Err(STATUS_IO);
    }

    let mut request = [0_u8; REQUEST_HEADER];
    let Some(length) = encode_request(&mut request, OP_MOUNTS, "", 0, 0, &[]) else {
        return Err(STATUS_INVALID);
    };
    if send_data(endpoint, VFS_REQUEST, &request[..length]) == u64::MAX {
        return Err(STATUS_IO);
    }

    let mut response = [0_u8; 4096];
    let (label, _word, _sender, length) = recv_data_from(own_endpoint, endpoint, &mut response);
    if label != VFS_RESPONSE {
        return Err(STATUS_IO);
    }
    let response = decode_response(&response[..length]).ok_or(STATUS_INVALID)?;
    if response.status != STATUS_OK {
        return Err(response.status);
    }
    let copy = response.data.len().min(out.len());
    out[..copy].copy_from_slice(&response.data[..copy]);
    Ok(response.data.len())
}

/// 解码后的请求。
pub struct Request<'a> {
    /// 操作码。
    pub op: u8,
    /// 路径。
    pub path: &'a str,
    /// 第一个参数（句柄、flags 等）。
    pub arg0: u64,
    /// 第二个参数（offset、size 等）。
    pub arg1: u64,
    /// 附加数据。
    pub data: &'a [u8],
}

/// 编码请求。
pub fn encode_request(
    buffer: &mut [u8],
    op: u8,
    path: &str,
    arg0: u64,
    arg1: u64,
    data: &[u8],
) -> Option<usize> {
    wire::encode_request(
        buffer,
        RequestHeader::new(op, 0, arg0, arg1),
        path.as_bytes(),
        data,
    )
}

/// 解码请求。
pub fn decode_request(bytes: &[u8]) -> Option<Request<'_>> {
    let request = wire::decode_request(bytes)?;
    Some(Request {
        op: request.header.op,
        path: core::str::from_utf8(request.text).ok()?,
        arg0: request.header.arg0,
        arg1: request.header.arg1,
        data: request.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_requests_and_responses() {
        let mut buffer = [0_u8; 64];
        let length =
            encode_request(&mut buffer, OP_WRITE, "/tmp/file", 7, 12, b"data").expect("编码");
        let request = decode_request(&buffer[..length]).expect("解码");
        assert_eq!(request.op, OP_WRITE);
        assert_eq!(request.path, "/tmp/file");
        assert_eq!(request.arg0, 7);
        assert_eq!(request.arg1, 12);
        assert_eq!(request.data, b"data");

        let length = encode_response(&mut buffer, STATUS_OK, b"ok").expect("编码响应");
        let response = decode_response(&buffer[..length]).expect("解码响应");
        assert_eq!(response.status, STATUS_OK);
        assert_eq!(response.data, b"ok");
    }
}
