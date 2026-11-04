//! VFS 客户端/服务端共享的简单线性协议。

/// 请求标签。
pub const VFS_REQUEST: u64 = 0x500;
/// 成功响应标签。
pub const VFS_RESPONSE: u64 = 0x501;

/// 查询元数据。
pub const OP_STAT: u8 = 1;
/// 读取整个文件。
pub const OP_READ: u8 = 2;
/// 从偏移 0 写入整个文件。
pub const OP_WRITE: u8 = 3;
/// 创建目录。
pub const OP_MKDIR: u8 = 4;
/// 列出目录。
pub const OP_LIST: u8 = 5;

/// 成功。
pub const STATUS_OK: u8 = 0;
/// 未找到。
pub const STATUS_NOT_FOUND: u8 = 1;
/// 参数非法。
pub const STATUS_INVALID: u8 = 2;
/// 后端或 IO 错误。
pub const STATUS_ERROR: u8 = 3;

/// 请求头：op + path_len。
const REQUEST_HEADER: usize = 3;
/// 响应头：status + data_len。
const RESPONSE_HEADER: usize = 3;

/// 解码后的请求。
pub struct Request<'a> {
    /// 操作码。
    pub op: u8,
    /// 路径。
    pub path: &'a str,
    /// 附加数据。
    pub data: &'a [u8],
}

/// 解码后的响应。
pub struct Response<'a> {
    /// 状态码。
    pub status: u8,
    /// 数据。
    pub data: &'a [u8],
}

/// 编码请求，返回写入长度；缓冲区不足时返回 `None`。
pub fn encode_request(buffer: &mut [u8], op: u8, path: &str, data: &[u8]) -> Option<usize> {
    let path = path.as_bytes();
    if path.len() > u16::MAX as usize {
        return None;
    }
    let end = REQUEST_HEADER
        .checked_add(path.len())?
        .checked_add(data.len())?;
    if end > buffer.len() {
        return None;
    }
    buffer[0] = op;
    buffer[1..3].copy_from_slice(&(path.len() as u16).to_le_bytes());
    buffer[3..3 + path.len()].copy_from_slice(path);
    buffer[3 + path.len()..end].copy_from_slice(data);
    Some(end)
}

/// 解码请求。
pub fn decode_request(bytes: &[u8]) -> Option<Request<'_>> {
    if bytes.len() < REQUEST_HEADER {
        return None;
    }
    let length = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
    let path_end = REQUEST_HEADER.checked_add(length)?;
    if path_end > bytes.len() {
        return None;
    }
    let path = core::str::from_utf8(&bytes[REQUEST_HEADER..path_end]).ok()?;
    Some(Request {
        op: bytes[0],
        path,
        data: &bytes[path_end..],
    })
}

/// 编码响应，返回写入长度；缓冲区不足时返回 `None`。
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
    fn round_trips_requests_and_responses() {
        let mut buffer = [0_u8; 64];
        let length = encode_request(&mut buffer, OP_WRITE, "/tmp/file", b"data").expect("编码");
        let request = decode_request(&buffer[..length]).expect("解码");
        assert_eq!(request.op, OP_WRITE);
        assert_eq!(request.path, "/tmp/file");
        assert_eq!(request.data, b"data");

        let length = encode_response(&mut buffer, STATUS_OK, b"ok").expect("编码响应");
        let response = decode_response(&buffer[..length]).expect("解码响应");
        assert_eq!(response.status, STATUS_OK);
        assert_eq!(response.data, b"ok");
    }
}
