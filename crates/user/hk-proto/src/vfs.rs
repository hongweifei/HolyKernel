//! VFS 客户端/服务端共享协议与固定缓冲客户端。

use crate::status::RESPONSE_HEADER;
use hk_server_api::{
    MemoryClient, RIGHT_ADMIN, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, close_handle, frame_map,
    handle_duplicate, recv_data_from, region_info, region_unmap, send_data, send_handle,
    service_endpoint,
};

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

/// 请求头：op + path_len + arg0 + arg1。
const REQUEST_HEADER: usize = 1 + 2 + 8 + 8;

/// 单条 VFS 协议消息的缓冲上限。
const BUFFER: usize = 64 * 1024;

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

/// 固定缓冲 VFS 客户端。
pub struct VfsClient {
    service: &'static str,
    endpoint: u64,
    own_endpoint: u64,
    request: [u8; BUFFER],
    response: [u8; BUFFER],
    frame: Option<u64>,
    region: u64,
    mapping_virt: u64,
    attached: u64,
}

impl Drop for VfsClient {
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take() {
            let endpoint = service_endpoint(self.service);
            if endpoint != 0 {
                let _ = send_data(endpoint, VFS_DETACH, &[]);
            }
            let _ = region_unmap(self.region);
            let _ = close_handle(frame);
        }
    }
}

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

impl VfsClient {
    /// 构造客户端；`service` 是 VFS 服务名，`own_endpoint` 是本任务端点。
    pub const fn new(service: &'static str, own_endpoint: u64) -> Self {
        Self {
            service,
            endpoint: 0,
            own_endpoint,
            request: [0; BUFFER],
            response: [0; BUFFER],
            frame: None,
            region: 0,
            mapping_virt: 0,
            attached: 0,
        }
    }

    fn ensure_attach(&mut self) -> Result<u64, u8> {
        let endpoint = service_endpoint(self.service);
        if endpoint == 0 {
            return Err(STATUS_IO);
        }
        if self.attached == endpoint && self.frame.is_some() {
            return Ok(endpoint);
        }
        if self.frame.is_none() {
            let Some(frame) = MemoryClient::new(self.own_endpoint).frame(VFS_BUFFER_LEN) else {
                return Err(STATUS_IO);
            };
            let attach = handle_duplicate(
                frame,
                RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER | RIGHT_ADMIN,
            );
            if attach == u64::MAX {
                let _ = close_handle(frame);
                return Err(STATUS_IO);
            }
            let Some(region) = frame_map(frame, RIGHT_READ | RIGHT_WRITE, 0) else {
                let _ = close_handle(frame);
                let _ = close_handle(attach);
                return Err(STATUS_IO);
            };
            let Some((virt, _)) = region_info(region) else {
                let _ = region_unmap(region);
                let _ = close_handle(attach);
                return Err(STATUS_IO);
            };
            self.frame = Some(attach);
            self.region = region;
            self.mapping_virt = virt;
        }
        let number = self.frame.ok_or(STATUS_IO)?;
        let duplicate = handle_duplicate(
            number,
            RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER | RIGHT_ADMIN,
        );
        if duplicate == u64::MAX {
            return Err(STATUS_IO);
        }
        if send_handle(endpoint, VFS_ATTACH, VFS_BUFFER_LEN as u64, duplicate) == u64::MAX {
            return Err(STATUS_IO);
        }
        self.attached = endpoint;
        Ok(endpoint)
    }

    /// 返回客户端共享帧。
    pub fn buffer(&mut self) -> &mut [u8] {
        // SAFETY: SYS_REGION_MAP 把 VFS_BUFFER_LEN 字节映射到自动分配的地址。
        unsafe { core::slice::from_raw_parts_mut(self.mapping_virt as *mut u8, VFS_BUFFER_LEN) }
    }

    /// 从共享帧读取文件内容，返回字节数。
    pub fn read_buffer(&mut self, handle: u64, offset: u64, length: usize) -> Result<usize, u8> {
        if length > VFS_BUFFER_LEN {
            return Err(STATUS_INVALID);
        }
        self.ensure_attach()?;
        let argument = (length as u64).to_le_bytes();
        let response = self.exchange(OP_READ_BUFFER, "", handle, offset, &argument)?;
        if response < 8 {
            return Err(STATUS_INVALID);
        }
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + 8]);
        Ok(u64::from_le_bytes(bytes) as usize)
    }

    /// 把共享帧中的内容写入文件，返回字节数。
    pub fn write_buffer(&mut self, handle: u64, offset: u64, data: &[u8]) -> Result<usize, u8> {
        if data.len() > VFS_BUFFER_LEN {
            return Err(STATUS_INVALID);
        }
        self.ensure_attach()?;
        self.buffer()[..data.len()].copy_from_slice(data);
        let argument = (data.len() as u64).to_le_bytes();
        let response = self.exchange(OP_WRITE_BUFFER, "", handle, offset, &argument)?;
        if response < 8 {
            return Err(STATUS_INVALID);
        }
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + 8]);
        Ok(u64::from_le_bytes(bytes) as usize)
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

    /// 卸载挂载点。
    pub fn umount(&mut self, path: &str) -> Result<(), u8> {
        self.exchange(OP_UMOUNT, path, 0, 0, &[]).map(|_| ())
    }

    /// 刷写后端挂起写入。
    pub fn sync(&mut self) -> Result<(), u8> {
        self.exchange(OP_SYNC, "", 0, 0, &[]).map(|_| ())
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
        let endpoint = service_endpoint(self.service);
        if endpoint == 0 {
            return Err(STATUS_IO);
        }
        self.endpoint = endpoint;
        if send_data(self.endpoint, VFS_REQUEST, &self.request[..length]) == u64::MAX {
            return Err(STATUS_IO);
        }
        let (label, _word, _sender, length) =
            recv_data_from(self.own_endpoint, self.endpoint, &mut self.response);
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
