//! 轻量内容压缩：游程编码，无外部依赖。

use alloc::vec::Vec;

use crate::error::HolyError;

const LITERAL: u8 = 0;
const REPEAT: u8 = 1;
const HEADER: usize = 4;

/// 压缩内容并加长度头；结果不优于原文时返回 `None`。
pub(crate) fn compress(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() {
        return None;
    }
    let mut stream = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        let mut run = 1;
        while index + run < data.len() && data[index + run] == data[index] && run < 255 {
            run += 1;
        }
        if run >= 4 {
            stream.push(REPEAT);
            stream.push(run as u8);
            stream.push(data[index]);
            index += run;
            continue;
        }
        let start = index;
        index += run;
        while index < data.len() && index - start < 255 {
            let mut next = 1;
            while index + next < data.len() && data[index + next] == data[index] && next < 255 {
                next += 1;
            }
            if next >= 4 {
                break;
            }
            index += next;
        }
        let length = index - start;
        stream.push(LITERAL);
        stream.push(length as u8);
        stream.extend_from_slice(&data[start..index]);
    }
    let total = HEADER + stream.len();
    if total >= data.len() {
        return None;
    }
    let mut payload = Vec::with_capacity(total);
    payload.extend_from_slice(&(stream.len() as u32).to_le_bytes());
    payload.extend_from_slice(&stream);
    Some(payload)
}

/// 解压带长度头的内容，并校验明文长度。
pub(crate) fn decompress(payload: &[u8], size: usize) -> Result<Vec<u8>, HolyError> {
    if payload.len() < HEADER {
        return Err(HolyError::Corrupt);
    }
    let length = u32::from_le_bytes(
        payload[..HEADER]
            .try_into()
            .map_err(|_| HolyError::Corrupt)?,
    ) as usize;
    let stream = payload
        .get(HEADER..HEADER + length)
        .ok_or(HolyError::Corrupt)?;
    let mut output = Vec::with_capacity(size);
    let mut index = 0;
    while index < stream.len() {
        let tag = stream[index];
        index += 1;
        let count = *stream.get(index).ok_or(HolyError::Corrupt)? as usize;
        index += 1;
        match tag {
            LITERAL => {
                let bytes = stream.get(index..index + count).ok_or(HolyError::Corrupt)?;
                output.extend_from_slice(bytes);
                index += count;
            }
            REPEAT => {
                let byte = *stream.get(index).ok_or(HolyError::Corrupt)?;
                index += 1;
                output.extend(core::iter::repeat_n(byte, count));
            }
            _ => return Err(HolyError::Corrupt),
        }
        if output.len() > size {
            return Err(HolyError::Corrupt);
        }
    }
    if output.len() != size {
        return Err(HolyError::Corrupt);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{compress, decompress};

    #[test]
    fn round_trips_repetitive_data() {
        let data = alloc::vec![0x55; 4096];
        let payload = compress(&data).expect("压缩");
        assert!(payload.len() < data.len());
        assert_eq!(decompress(&payload, data.len()).expect("解压"), data);
    }

    #[test]
    fn skips_unhelpful_compression() {
        let data = [0_u8, 1, 2, 3, 4, 5, 6, 7];
        assert!(compress(&data).is_none());
    }
}
