//! 块设备请求/响应协议与客户端。

use crate::{recv_data_from, send_data, service_endpoint};

/// 请求标签。
pub const BLOCK_REQUEST: u64 = 0x600;
/// 响应标签。
pub const BLOCK_RESPONSE: u64 = 0x601;
/// DeviceGrant 移交标签。
pub const BLOCK_GRANT: u64 = 0x602;
/// 没有块设备标签。
pub const BLOCK_NO_DEVICE: u64 = 0x603;

/// 读取块。
pub const OP_READ: u8 = 1;
/// 写入块。
pub const OP_WRITE: u8 = 2;
/// 刷写。
pub const OP_FLUSH: u8 = 3;
/// 查询容量。
pub const OP_CAPACITY: u8 = 4;

/// 成功。
pub const STATUS_OK: u8 = 0;
/// 参数非法。
pub const STATUS_INVALID: u8 = 1;
/// 范围越界。
pub const STATUS_RANGE: u8 = 2;
/// I/O 错误。
pub const STATUS_ERROR: u8 = 3;

const REQUEST_HEADER: usize = 1 + 8 + 4;
const RESPONSE_HEADER: usize = 3;

/// 解码后的请求。
pub struct Request<'a> {
    /// 操作码。
    pub op: u8,
    /// 起始块。
    pub block: u64,
    /// 数据长度。
    pub length: u32,
    /// 写入数据。
    pub data: &'a [u8],
}

/// 编码请求。
pub fn encode_request(
    buffer: &mut [u8],
    op: u8,
    block: u64,
    length: u32,
    data: &[u8],
) -> Option<usize> {
    let end = REQUEST_HEADER.checked_add(data.len())?;
    if end > buffer.len() {
        return None;
    }
    buffer[0] = op;
    buffer[1..9].copy_from_slice(&block.to_le_bytes());
    buffer[9..13].copy_from_slice(&length.to_le_bytes());
    buffer[REQUEST_HEADER..end].copy_from_slice(data);
    Some(end)
}

/// 解码请求。
pub fn decode_request(bytes: &[u8]) -> Option<Request<'_>> {
    if bytes.len() < REQUEST_HEADER {
        return None;
    }
    Some(Request {
        op: bytes[0],
        block: u64::from_le_bytes(bytes[1..9].try_into().ok()?),
        length: u32::from_le_bytes(bytes[9..13].try_into().ok()?),
        data: &bytes[REQUEST_HEADER..],
    })
}

/// 编码响应。
pub fn encode_response(buffer: &mut [u8], status: u8, data: &[u8]) -> Option<usize> {
    let end = RESPONSE_HEADER.checked_add(data.len())?;
    if end > buffer.len() || data.len() > u16::MAX as usize {
        return None;
    }
    buffer[0] = status;
    buffer[1..3].copy_from_slice(&(data.len() as u16).to_le_bytes());
    buffer[3..end].copy_from_slice(data);
    Some(end)
}

/// 解码响应。
pub fn decode_response(bytes: &[u8]) -> Option<(u8, &[u8])> {
    if bytes.len() < RESPONSE_HEADER {
        return None;
    }
    let length = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
    let end = RESPONSE_HEADER.checked_add(length)?;
    (end <= bytes.len()).then_some((bytes[0], &bytes[RESPONSE_HEADER..end]))
}

/// 固定缓冲 Block 客户端。
pub struct BlockClient {
    service: &'static str,
    endpoint: u64,
    own_endpoint: u64,
    request: [u8; 8192],
    response: [u8; 8192],
}

impl BlockClient {
    /// 构造客户端。
    pub const fn new(service: &'static str, own_endpoint: u64) -> Self {
        Self {
            service,
            endpoint: 0,
            own_endpoint,
            request: [0; 8192],
            response: [0; 8192],
        }
    }

    fn exchange(&mut self, op: u8, block: u64, length: u32, data: &[u8]) -> Result<usize, u8> {
        let Some(request_length) = encode_request(&mut self.request, op, block, length, data)
        else {
            return Err(STATUS_INVALID);
        };
        let endpoint = service_endpoint(self.service);
        if endpoint == 0 {
            return Err(STATUS_ERROR);
        }
        self.endpoint = endpoint;
        if send_data(
            self.endpoint,
            BLOCK_REQUEST,
            &self.request[..request_length],
        ) == u64::MAX
        {
            return Err(STATUS_ERROR);
        }
        let (label, _word, _sender, response_length) =
            recv_data_from(self.own_endpoint, self.endpoint, &mut self.response);
        if label != BLOCK_RESPONSE {
            return Err(STATUS_ERROR);
        }
        let Some((status, body)) = decode_response(&self.response[..response_length]) else {
            return Err(STATUS_INVALID);
        };
        if status != STATUS_OK {
            return Err(status);
        }
        Ok(body.len())
    }

    /// 读取块数据。
    pub fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<usize, u8> {
        let length = buffer.len().min(u32::MAX as usize) as u32;
        let copied = self.exchange(OP_READ, block, length, &[])?;
        let copy = copied.min(buffer.len());
        buffer[..copy].copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + copy]);
        Ok(copy)
    }

    /// 写入块数据。
    pub fn write(&mut self, block: u64, data: &[u8]) -> Result<usize, u8> {
        self.exchange(OP_WRITE, block, data.len() as u32, data)
    }

    /// 刷写。
    pub fn flush(&mut self) -> Result<(), u8> {
        self.exchange(OP_FLUSH, 0, 0, &[]).map(|_| ())
    }

    /// 返回容量字节数。
    pub fn capacity(&mut self) -> Result<u64, u8> {
        let length = self.exchange(OP_CAPACITY, 0, 0, &[])?;
        if length < 8 {
            return Err(STATUS_INVALID);
        }
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&self.response[RESPONSE_HEADER..RESPONSE_HEADER + 8]);
        Ok(u64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_full_block_requests_and_responses() {
        let payload = [0xAB_u8; 4096];
        let mut buffer = [0_u8; 8192];

        let length = encode_request(&mut buffer, OP_WRITE, 7, 4096, &payload).expect("编码请求");
        let request = decode_request(&buffer[..length]).expect("解码请求");
        assert_eq!(request.op, OP_WRITE);
        assert_eq!(request.block, 7);
        assert_eq!(request.length, 4096);
        assert_eq!(request.data, payload);

        let length = encode_response(&mut buffer, STATUS_OK, &payload).expect("编码响应");
        let (status, body) = decode_response(&buffer[..length]).expect("解码响应");
        assert_eq!(status, STATUS_OK);
        assert_eq!(body, payload);
    }
}
