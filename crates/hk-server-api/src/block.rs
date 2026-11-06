//! 块设备请求/响应协议与客户端。
//!
//! 请求携带 `tag`，响应按 `tag` 回显，同一连接上可以同时存在多个未完成请求。
//! 数据面走共享 Frame：每个 `tag` 对应一个客户端帧槽位，服务端按发送者与槽位
//! 分别映射，因此多个请求可以并行提交、乱序完成。

/// 请求标签。
pub const BLOCK_REQUEST: u64 = 0x600;
/// 响应标签。
pub const BLOCK_RESPONSE: u64 = 0x601;
/// DeviceGrant 移交标签。
pub use crate::device::DEVICE_GRANT as BLOCK_GRANT;
/// 没有块设备标签。
pub use crate::device::DEVICE_NONE as BLOCK_NO_DEVICE;
/// 驱动请求下发设备授权。
pub use crate::device::DEVICE_REQUEST;
/// 请求块设备服务退出（用于故障注入与重启测试）。
pub const BLOCK_SHUTDOWN: u64 = 0x605;
/// 附加共享 Frame；消息字见 [`attach_word`]。
pub const BLOCK_ATTACH: u64 = 0x606;
/// 解除共享 Frame；消息字为槽位，`u64::MAX` 表示全部解除。
pub const BLOCK_DETACH: u64 = 0x607;

/// 每个客户端可用的共享帧槽位数量。
pub const FRAME_SLOTS: u32 = 8;
/// 块客户端共享 Frame 的固定映射地址基址。
pub const FRAME_VIRT: u64 = 0x7000_0000;
/// 相邻槽位的地址间隔。
pub const FRAME_STRIDE: u64 = 0x1_0000;
/// 单个共享 Frame 的长度。
pub const FRAME_LEN: usize = 60 * 1024;

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
/// 设备队列已满；调用方回收完成后再提交。
pub const STATUS_BUSY: u8 = 4;

const REQUEST_HEADER: usize = 1 + 8 + 4 + 4;
/// 响应头：status + data_len + tag。
pub const RESPONSE_HEADER: usize = 3 + 4;

/// 解码后的请求。
pub struct Request<'a> {
    /// 操作码。
    pub op: u8,
    /// 请求标签；同时选择共享帧槽位。
    pub tag: u32,
    /// 起始块。
    pub block: u64,
    /// 数据长度。
    pub length: u32,
    /// 写入数据。
    pub data: &'a [u8],
}

/// 组装 `BLOCK_ATTACH` 的消息字：高 32 位为槽位，低 32 位为长度。
pub const fn attach_word(slot: u32, length: usize) -> u64 {
    ((slot as u64) << 32) | (length as u64 & 0xFFFF_FFFF)
}

/// 从 `BLOCK_ATTACH` 消息字取出槽位。
pub const fn attach_slot(word: u64) -> u32 {
    (word >> 32) as u32
}

/// 从 `BLOCK_ATTACH` 消息字取出长度。
pub const fn attach_length(word: u64) -> usize {
    (word & 0xFFFF_FFFF) as usize
}

/// 编码请求。
#[allow(clippy::too_many_arguments)]
pub fn encode_request(
    buffer: &mut [u8],
    tag: u32,
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
    buffer[13..17].copy_from_slice(&tag.to_le_bytes());
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
        tag: u32::from_le_bytes(bytes[13..17].try_into().ok()?),
        data: &bytes[REQUEST_HEADER..],
    })
}

/// 编码响应。
pub fn encode_response(buffer: &mut [u8], tag: u32, status: u8, data: &[u8]) -> Option<usize> {
    let end = RESPONSE_HEADER.checked_add(data.len())?;
    if end > buffer.len() || data.len() > u16::MAX as usize {
        return None;
    }
    buffer[0] = status;
    buffer[1..3].copy_from_slice(&(data.len() as u16).to_le_bytes());
    buffer[3..7].copy_from_slice(&tag.to_le_bytes());
    buffer[7..end].copy_from_slice(data);
    Some(end)
}

/// 解码响应；返回状态、标签与载荷。
pub fn decode_response(bytes: &[u8]) -> Option<(u8, u32, &[u8])> {
    if bytes.len() < RESPONSE_HEADER {
        return None;
    }
    let length = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
    let end = RESPONSE_HEADER.checked_add(length)?;
    (end <= bytes.len()).then_some((
        bytes[0],
        u32::from_le_bytes(bytes[3..7].try_into().ok()?),
        &bytes[RESPONSE_HEADER..end],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_tagged_block_requests_and_responses() {
        let payload = [0xAB_u8; 4096];
        let mut buffer = [0_u8; 8192];

        let length = encode_request(&mut buffer, 3, OP_WRITE, 7, 4096, &payload).expect("编码请求");
        let request = decode_request(&buffer[..length]).expect("解码请求");
        assert_eq!(request.op, OP_WRITE);
        assert_eq!(request.tag, 3);
        assert_eq!(request.block, 7);
        assert_eq!(request.length, 4096);
        assert_eq!(request.data, payload);

        let length = encode_response(&mut buffer, 3, STATUS_OK, &payload).expect("编码响应");
        let (status, tag, body) = decode_response(&buffer[..length]).expect("解码响应");
        assert_eq!(status, STATUS_OK);
        assert_eq!(tag, 3);
        assert_eq!(body, payload);
    }

    #[test]
    fn packs_attach_slot_and_length() {
        let word = attach_word(5, FRAME_LEN);
        assert_eq!(attach_slot(word), 5);
        assert_eq!(attach_length(word), FRAME_LEN);
    }
}
