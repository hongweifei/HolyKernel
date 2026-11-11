//! VFS 客户端：收发缓冲按需分配，共享帧按需建立。

use alloc::vec::Vec;

use hk_proto::status::{RESPONSE_HEADER, STATUS_INVALID, STATUS_IO, STATUS_OK, decode_response};
use hk_proto::vfs::{
    OP_CLOSE, OP_LIST, OP_MKDIR, OP_MOUNTS, OP_OPEN, OP_READ, OP_READ_BUFFER, OP_STAT, OP_SYNC,
    OP_TRUNCATE, OP_UMOUNT, OP_UNLINK, OP_WRITE, OP_WRITE_BUFFER, VFS_ATTACH, VFS_BUFFER_LEN,
    VFS_DETACH, VFS_MESSAGE_LEN, VFS_REQUEST, VFS_RESPONSE, encode_request,
};
use hk_server_api::{
    CapabilityKind, MemoryClient, RIGHT_ADMIN, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE,
    close_handle, handle_duplicate, recv_data_from, region_unmap, send_data, send_handle,
    service_endpoint,
};

/// VFS 客户端；缓冲与共享帧在首次使用时建立。
pub struct VfsClient {
    service: &'static str,
    endpoint: u64,
    own_endpoint: u64,
    request: Vec<u8>,
    response: Vec<u8>,
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

impl VfsClient {
    /// 构造客户端；`service` 是 VFS 服务名，`own_endpoint` 是本任务端点。
    pub const fn new(service: &'static str, own_endpoint: u64) -> Self {
        Self {
            service,
            endpoint: 0,
            own_endpoint,
            request: Vec::new(),
            response: Vec::new(),
            frame: None,
            region: 0,
            mapping_virt: 0,
            attached: 0,
        }
    }

    fn ensure_buffers(&mut self) {
        if self.request.len() < VFS_MESSAGE_LEN {
            self.request.resize(VFS_MESSAGE_LEN, 0);
        }
        if self.response.len() < VFS_MESSAGE_LEN {
            self.response.resize(VFS_MESSAGE_LEN, 0);
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
            // 共享缓冲区是匿名能力：客户端与服务端都能映射同一份物理页，
            // 句柄不随映射失效。
            let Some((capability, region, virt)) =
                MemoryClient::new(self.own_endpoint).map(CapabilityKind::Anonymous, VFS_BUFFER_LEN)
            else {
                return Err(STATUS_IO);
            };
            self.frame = Some(capability);
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
        self.ensure_buffers();
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
    fn client_stays_small_and_allocates_lazily() {
        let mut client = VfsClient::new("hk-server-path", 1);
        assert!(core::mem::size_of::<VfsClient>() < 256);
        assert!(client.request.is_empty() && client.response.is_empty());
        client.ensure_buffers();
        assert_eq!(client.request.len(), VFS_MESSAGE_LEN);
        assert_eq!(client.response.len(), VFS_MESSAGE_LEN);
    }
}
