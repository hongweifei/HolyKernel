//! 服务状态码与响应封装。
//!
//! 所有服务协议共用一套状态码与 `status + data_len + data` 响应形状；需要
//! 关联编号的协议（块设备）在响应头后追加固定 `extra`。各协议的请求形状与
//! 操作码仍留在自己的模块里。

use crate::wire;

pub use hk_status::Status;

/// 响应头长度：status + data_len。
pub const RESPONSE_HEADER: usize = wire::RESPONSE_HEADER;

/// 成功。
pub const STATUS_OK: u8 = Status::Ok.as_u8();
/// 未找到。
pub const STATUS_NOT_FOUND: u8 = Status::NotFound.as_u8();
/// 参数非法。
pub const STATUS_INVALID: u8 = Status::Invalid.as_u8();
/// 后端错误。
pub const STATUS_ERROR: u8 = Status::Error.as_u8();
/// 服务不可用。
pub const STATUS_IO: u8 = Status::Io.as_u8();
/// 权限不足。
pub const STATUS_PERMISSION: u8 = Status::Permission.as_u8();
/// 已存在。
pub const STATUS_EXISTS: u8 = Status::Exists.as_u8();
/// 期望值或令牌代际已过期。
pub const STATUS_STALE: u8 = Status::Stale.as_u8();
/// 卷已损坏并进入只读。
pub const STATUS_DAMAGED: u8 = Status::Damaged.as_u8();
/// 卷当前只读。
pub const STATUS_READ_ONLY: u8 = Status::ReadOnly.as_u8();
/// 访问范围越界。
pub const STATUS_RANGE: u8 = Status::Range.as_u8();
/// 队列已满，回收完成后再提交。
pub const STATUS_BUSY: u8 = Status::Busy.as_u8();

/// 解码后的响应。
pub struct Response<'a> {
    /// 状态码。
    pub status: u8,
    /// 数据。
    pub data: &'a [u8],
}

/// 编码响应。
pub fn encode_response(buffer: &mut [u8], status: u8, data: &[u8]) -> Option<usize> {
    wire::encode_response(buffer, status, &[], data)
}

/// 解码响应。
pub fn decode_response(bytes: &[u8]) -> Option<Response<'_>> {
    let response = wire::decode_response(bytes, 0)?;
    Some(Response {
        status: response.status,
        data: response.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_roundtrips() {
        let mut buffer = [0_u8; 16];
        let length = encode_response(&mut buffer, STATUS_NOT_FOUND, &[1, 2, 3]).expect("编码");
        let response = decode_response(&buffer[..length]).expect("解码");
        assert_eq!(response.status, STATUS_NOT_FOUND);
        assert_eq!(response.data, &[1, 2, 3]);

        assert!(decode_response(&[0, 9]).is_none());
        let mut small = [0_u8; 4];
        assert!(encode_response(&mut small, STATUS_OK, &[0; 8]).is_none());
    }
}
