//! 协议公共帧：请求头与响应头。
//!
//! 所有服务协议共用同一套帧头：
//! - 请求头 `[op][kind][text_len u16][arg0 u64][arg1 u64]`，后接可选文本与数据；
//! - 响应头 `[status][data_len u16][extra]`，`extra` 供需要关联编号的协议使用。
//!
//! 协议模块只解释字段语义、操作码与 `extra` 布局，不再各自维护头编解码。

/// 请求头长度。
pub const REQUEST_HEADER: usize = 1 + 1 + 2 + 8 + 8;
/// 响应头基础长度；不含协议自定义的 `extra`。
pub const RESPONSE_HEADER: usize = 3;

/// 请求头。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestHeader {
    /// 操作码。
    pub op: u8,
    /// 协议自定义的单字节类型；不需要时填 0。
    pub kind: u8,
    /// 第一个参数。
    pub arg0: u64,
    /// 第二个参数。
    pub arg1: u64,
}

impl RequestHeader {
    /// 构造请求头。
    pub const fn new(op: u8, kind: u8, arg0: u64, arg1: u64) -> Self {
        Self {
            op,
            kind,
            arg0,
            arg1,
        }
    }
}

/// 解码后的请求。
pub struct DecodedRequest<'a> {
    /// 帧头。
    pub header: RequestHeader,
    /// 可选文本；长度为 0 时为空切片。
    pub text: &'a [u8],
    /// 附加数据。
    pub data: &'a [u8],
}

/// 编码请求；返回整帧长度。
pub fn encode_request(
    buffer: &mut [u8],
    header: RequestHeader,
    text: &[u8],
    data: &[u8],
) -> Option<usize> {
    if text.len() > u16::MAX as usize {
        return None;
    }
    let end = REQUEST_HEADER
        .checked_add(text.len())?
        .checked_add(data.len())?;
    if end > buffer.len() {
        return None;
    }
    buffer[0] = header.op;
    buffer[1] = header.kind;
    buffer[2..4].copy_from_slice(&(text.len() as u16).to_le_bytes());
    buffer[4..12].copy_from_slice(&header.arg0.to_le_bytes());
    buffer[12..20].copy_from_slice(&header.arg1.to_le_bytes());
    buffer[REQUEST_HEADER..REQUEST_HEADER + text.len()].copy_from_slice(text);
    buffer[REQUEST_HEADER + text.len()..end].copy_from_slice(data);
    Some(end)
}

/// 解码请求。
pub fn decode_request(bytes: &[u8]) -> Option<DecodedRequest<'_>> {
    if bytes.len() < REQUEST_HEADER {
        return None;
    }
    let text_len = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    let text_end = REQUEST_HEADER.checked_add(text_len)?;
    if text_end > bytes.len() {
        return None;
    }
    Some(DecodedRequest {
        header: RequestHeader {
            op: bytes[0],
            kind: bytes[1],
            arg0: u64::from_le_bytes(bytes[4..12].try_into().ok()?),
            arg1: u64::from_le_bytes(bytes[12..20].try_into().ok()?),
        },
        text: &bytes[REQUEST_HEADER..text_end],
        data: &bytes[text_end..],
    })
}

/// 解码后的响应。
pub struct DecodedResponse<'a> {
    /// 状态码。
    pub status: u8,
    /// 协议自定义扩展；长度由解码方声明。
    pub extra: &'a [u8],
    /// 载荷。
    pub data: &'a [u8],
}

/// 编码响应；`extra` 写在响应头与载荷之间，返回整帧长度。
pub fn encode_response(buffer: &mut [u8], status: u8, extra: &[u8], data: &[u8]) -> Option<usize> {
    if data.len() > u16::MAX as usize {
        return None;
    }
    let data_start = RESPONSE_HEADER.checked_add(extra.len())?;
    let end = data_start.checked_add(data.len())?;
    if end > buffer.len() {
        return None;
    }
    buffer[0] = status;
    buffer[1..3].copy_from_slice(&(data.len() as u16).to_le_bytes());
    buffer[RESPONSE_HEADER..data_start].copy_from_slice(extra);
    buffer[data_start..end].copy_from_slice(data);
    Some(end)
}

/// 解码响应；`extra_len` 为该协议固定声明的扩展长度。
pub fn decode_response(bytes: &[u8], extra_len: usize) -> Option<DecodedResponse<'_>> {
    let data_start = RESPONSE_HEADER.checked_add(extra_len)?;
    if bytes.len() < data_start {
        return None;
    }
    let length = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
    let end = data_start.checked_add(length)?;
    if end > bytes.len() {
        return None;
    }
    Some(DecodedResponse {
        status: bytes[0],
        extra: &bytes[RESPONSE_HEADER..data_start],
        data: &bytes[data_start..end],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_request_with_text_and_data() {
        let mut buffer = [0_u8; 64];
        let header = RequestHeader::new(7, 3, 0x1122, 0x3344);
        let length = encode_request(&mut buffer, header, b"path", b"body").expect("编码");
        let request = decode_request(&buffer[..length]).expect("解码");
        assert_eq!(request.header, header);
        assert_eq!(request.text, b"path");
        assert_eq!(request.data, b"body");
    }

    #[test]
    fn round_trips_response_with_and_without_extra() {
        let mut buffer = [0_u8; 32];
        let length = encode_response(&mut buffer, 5, &[], b"data").expect("编码");
        let response = decode_response(&buffer[..length], 0).expect("解码");
        assert_eq!(response.status, 5);
        assert!(response.extra.is_empty());
        assert_eq!(response.data, b"data");

        let tag = 9_u32.to_le_bytes();
        let length = encode_response(&mut buffer, 6, &tag, b"x").expect("编码");
        let response = decode_response(&buffer[..length], 4).expect("解码");
        assert_eq!(response.status, 6);
        assert_eq!(response.extra, tag);
        assert_eq!(response.data, b"x");
    }

    #[test]
    fn rejects_truncated_headers() {
        assert!(decode_request(&[0; REQUEST_HEADER - 1]).is_none());
        assert!(decode_response(&[0; RESPONSE_HEADER - 1], 0).is_none());
        assert!(decode_response(&[0; RESPONSE_HEADER], 4).is_none());
    }
}
