//! 服务响应框架：状态码与 `status + data_len + data` 编解码。
//!
//! VFS 与 Holy 文档协议共用这套响应形状；各协议的请求形状与操作码仍留在
//! 自己的模块里。

/// 响应头长度：status + data_len。
pub(crate) const RESPONSE_HEADER: usize = 3;

/// 成功。
pub const STATUS_OK: u8 = 0;
/// 未找到。
pub const STATUS_NOT_FOUND: u8 = 1;
/// 参数非法。
pub const STATUS_INVALID: u8 = 2;
/// 后端错误。
pub const STATUS_ERROR: u8 = 3;
/// 服务不可用。
pub const STATUS_IO: u8 = 4;
/// 权限不足。
pub const STATUS_PERMISSION: u8 = 5;
/// 已存在。
pub const STATUS_EXISTS: u8 = 6;
/// 期望值或令牌代际已过期。
pub const STATUS_STALE: u8 = 7;
/// 卷已损坏并进入只读。
pub const STATUS_DAMAGED: u8 = 8;
/// 卷当前只读。
pub const STATUS_READ_ONLY: u8 = 9;

/// 解码后的响应。
pub struct Response<'a> {
    /// 状态码。
    pub status: u8,
    /// 数据。
    pub data: &'a [u8],
}

/// 编码响应。
pub fn encode_response(buffer: &mut [u8], status: u8, data: &[u8]) -> Option<usize> {
    if data.len() > u16::MAX as usize {
        return None;
    }
    let end = RESPONSE_HEADER.checked_add(data.len())?;
    if end > buffer.len() {
        return None;
    }
    buffer[0] = status;
    buffer[1..3].copy_from_slice(&(data.len() as u16).to_le_bytes());
    buffer[3..end].copy_from_slice(data);
    Some(end)
}

/// 解码响应。
pub fn decode_response(bytes: &[u8]) -> Option<Response<'_>> {
    if bytes.len() < RESPONSE_HEADER {
        return None;
    }
    let length = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
    let end = RESPONSE_HEADER.checked_add(length)?;
    if end > bytes.len() {
        return None;
    }
    Some(Response {
        status: bytes[0],
        data: &bytes[RESPONSE_HEADER..end],
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
