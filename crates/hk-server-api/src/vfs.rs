//! VFS 客户端/服务端共享协议与固定缓冲客户端。

use crate::{recv_data, send_data};

/// 请求标签。
pub const VFS_REQUEST: u64 = 0x500;
/// 响应标签。
pub const VFS_RESPONSE: u64 = 0x501;

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
/// 在路径上挂载新的 tmpfs。
pub const OP_MOUNT_TMPFS: u8 = 10;
/// 列出挂载点。
pub const OP_MOUNTS: u8 = 11;
/// 卸载挂载点。
pub const OP_UMOUNT: u8 = 12;

/// 打开读权限。
pub const OPEN_READ: u64 = 1 << 0;
/// 打开写权限。
pub const OPEN_WRITE: u64 = 1 << 1;
/// 不存在时创建。
pub const OPEN_CREATE: u64 = 1 << 2;

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

/// 请求头：op + path_len + arg0 + arg1。
const REQUEST_HEADER: usize = 1 + 2 + 8 + 8;
/// 响应头：status + data_len。
const RESPONSE_HEADER: usize = 3;

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

/// 解码后的响应。
pub struct Response<'a> {
    /// 状态码。
    pub status: u8,
    /// 数据。
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
    buffer[3..11].copy_from_slice(&arg0.to_le_bytes());
    buffer[11..19].copy_from_slice(&arg1.to_le_bytes());
    buffer[19..19 + path.len()].copy_from_slice(path);
    buffer[19 + path.len()..end].copy_from_slice(data);
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
    let arg0 = u64::from_le_bytes(bytes[3..11].try_into().ok()?);
    let arg1 = u64::from_le_bytes(bytes[11..19].try_into().ok()?);
    Some(Request {
        op: bytes[0],
        path,
        arg0,
        arg1,
        data: &bytes[path_end..],
    })
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

/// 固定缓冲 VFS 客户端。
pub struct VfsClient {
    endpoint: u64,
    own_endpoint: u64,
    request: [u8; 4096],
    response: [u8; 4096],
}

impl VfsClient {
    /// 构造客户端；`endpoint` 是 VFS 服务端点，`own_endpoint` 是本任务端点。
    pub const fn new(endpoint: u64, own_endpoint: u64) -> Self {
        Self {
            endpoint,
            own_endpoint,
            request: [0; 4096],
            response: [0; 4096],
        }
    }

    /// 打开文件，返回句柄。
    pub fn open(&mut self, path: &str, flags: u64) -> Result<u64, u8> {
        let length = self.exchange(OP_OPEN, path, flags, 0, &[])?;
        if length < 8 {
            return Err(STATUS_INVALID);
        }
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + 8]);
        Ok(u64::from_le_bytes(bytes))
    }

    /// 关闭句柄。
    pub fn close(&mut self, handle: u64) -> Result<(), u8> {
        self.exchange(OP_CLOSE, "", handle, 0, &[]).map(|_| ())
    }

    /// 从偏移读取，返回实际拷贝到缓冲区的长度。
    pub fn read(&mut self, handle: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_READ, "", handle, offset, &[])?;
        let copy = length.min(buffer.len());
        buffer[..copy].copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + copy]);
        Ok(copy)
    }

    /// 从偏移写入。
    pub fn write(&mut self, handle: u64, offset: u64, data: &[u8]) -> Result<usize, u8> {
        self.exchange(OP_WRITE, "", handle, offset, data)?;
        Ok(data.len())
    }

    /// 查询元数据，返回 `(kind, size)`。
    pub fn stat(&mut self, path: &str) -> Result<(u8, u64), u8> {
        let length = self.exchange(OP_STAT, path, 0, 0, &[])?;
        if length < 9 {
            return Err(STATUS_INVALID);
        }
        let kind = self.response[RESPONSE_HEADER];
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&self.response[RESPONSE_HEADER + 1..RESPONSE_HEADER + 9]);
        Ok((kind, u64::from_le_bytes(bytes)))
    }

    /// 调整文件大小。
    pub fn truncate(&mut self, path: &str, size: u64) -> Result<(), u8> {
        self.exchange(OP_TRUNCATE, path, size, 0, &[]).map(|_| ())
    }

    /// 删除文件或空目录。
    pub fn unlink(&mut self, path: &str) -> Result<(), u8> {
        self.exchange(OP_UNLINK, path, 0, 0, &[]).map(|_| ())
    }

    /// 读取目录，返回服务端名称列表长度。
    pub fn readdir(&mut self, path: &str, buffer: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_LIST, path, 0, 0, &[])?;
        let copy = length.min(buffer.len());
        buffer[..copy].copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + copy]);
        Ok(length)
    }

    /// 创建目录。
    pub fn mkdir(&mut self, path: &str) -> Result<(), u8> {
        self.exchange(OP_MKDIR, path, 0, 0, &[]).map(|_| ())
    }

    /// 在路径上挂载新的 tmpfs。
    pub fn mount_tmpfs(&mut self, path: &str) -> Result<(), u8> {
        self.exchange(OP_MOUNT_TMPFS, path, 0, 0, &[]).map(|_| ())
    }

    /// 卸载挂载点。
    pub fn umount(&mut self, path: &str) -> Result<(), u8> {
        self.exchange(OP_UMOUNT, path, 0, 0, &[]).map(|_| ())
    }

    /// 列出挂载点。
    pub fn mounts(&mut self, buffer: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_MOUNTS, "", 0, 0, &[])?;
        let copy = length.min(buffer.len());
        buffer[..copy].copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + copy]);
        Ok(length)
    }

    fn exchange(
        &mut self,
        op: u8,
        path: &str,
        arg0: u64,
        arg1: u64,
        data: &[u8],
    ) -> Result<usize, u8> {
        let Some(length) = encode_request(&mut self.request, op, path, arg0, arg1, data) else {
            return Err(STATUS_INVALID);
        };
        if send_data(self.endpoint, VFS_REQUEST, &self.request[..length]) == u64::MAX {
            return Err(STATUS_IO);
        }
        let (label, _word, _sender, length) = recv_data(self.own_endpoint, &mut self.response);
        if label != VFS_RESPONSE {
            return Err(STATUS_IO);
        }
        let response = decode_response(&self.response[..length]).ok_or(STATUS_INVALID)?;
        if response.status != STATUS_OK {
            return Err(response.status);
        }
        Ok(response.data.len())
    }
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
