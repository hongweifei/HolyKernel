//! 内核压缩交换：把匿名页压成更小的字节块，归还物理页。
//!
//! 这是 HMP 的内存内 swap 层。格式是一个简化的 LZ4 block：
//! 每个序列先写 token，高 4 位是字面量长度、低 4 位是匹配长度减 4；
//! 15 表示后续用 255 累加扩展；字面量之后是 u16 小端匹配偏移。
//! 压缩结果只在严格小于原页时才被保留，否则调用方保留原页。

use alloc::vec::Vec;

/// 页大小；压缩单元固定为一页。
pub const BLOCK: usize = 4096;
/// 压缩结果至少要比原页小这么多才值得换出（元数据/查找成本）。
pub const MIN_GAIN: usize = 256;
const HASH_BITS: usize = 10;
const HASH_SIZE: usize = 1 << HASH_BITS;

fn hash(sequence: u32) -> usize {
    let mixed = sequence.wrapping_mul(2_654_435_761);
    (mixed >> (32 - HASH_BITS)) as usize
}

fn emit_length(out: &mut Vec<u8>, mut length: usize) {
    while length >= 255 {
        out.push(255);
        length -= 255;
    }
    out.push(length as u8);
}

/// 压缩一页；无可观收益时返回 `None`。
pub fn compress(source: &[u8]) -> Option<Vec<u8>> {
    if source.len() != BLOCK {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(BLOCK / 2).ok()?;
    let mut table = [0_u32; HASH_SIZE];
    let mut anchor = 0;
    let mut position = 0;
    while position + 4 <= source.len() {
        let sequence = u32::from_le_bytes(source[position..position + 4].try_into().ok()?);
        let slot = hash(sequence);
        let candidate = table[slot];
        table[slot] = position as u32 + 1;
        if candidate == 0 {
            position += 1;
            continue;
        }
        let candidate = (candidate - 1) as usize;
        if candidate >= position
            || source.get(candidate..candidate + 4) != source.get(position..position + 4)
        {
            position += 1;
            continue;
        }
        let mut length = 4;
        while position + length < source.len()
            && source[candidate + length] == source[position + length]
            && length < 0xFFF
        {
            length += 1;
        }
        // 字面量 token 与扩展长度。
        let literals = position - anchor;
        let literal_nibble = literals.min(15) as u8;
        let match_nibble = (length - 4).min(15) as u8;
        out.push((literal_nibble << 4) | match_nibble);
        if literals >= 15 {
            emit_length(&mut out, literals - 15);
        }
        out.extend_from_slice(&source[anchor..position]);
        out.extend_from_slice(&((position - candidate) as u16).to_le_bytes());
        if length - 4 >= 15 {
            emit_length(&mut out, length - 4 - 15);
        }
        position += length;
        anchor = position;
        if out.len() + (source.len() - anchor) + 16 >= source.len() {
            return None;
        }
    }
    // 末尾字面量是最后一个序列，没有偏移。
    let literals = source.len() - anchor;
    let literal_nibble = literals.min(15) as u8;
    out.push(literal_nibble << 4);
    if literals >= 15 {
        emit_length(&mut out, literals - 15);
    }
    out.extend_from_slice(&source[anchor..]);
    (out.len() + MIN_GAIN < source.len()).then_some(out)
}

/// 解压到固定页；格式错误或越界时返回 `None`。
pub fn decompress(source: &[u8], out: &mut [u8]) -> Option<()> {
    if out.len() != BLOCK {
        return None;
    }
    let mut input = 0_usize;
    let mut output = 0_usize;
    while input < source.len() {
        let token = *source.get(input)?;
        input += 1;
        let mut literals = usize::from(token >> 4);
        if literals == 15 {
            loop {
                let byte = *source.get(input)?;
                input += 1;
                literals = literals.checked_add(usize::from(byte))?;
                if byte != 255 {
                    break;
                }
            }
        }
        let literal_end = input.checked_add(literals)?;
        let destination_end = output.checked_add(literals)?;
        if literal_end > source.len() || destination_end > out.len() {
            return None;
        }
        out[output..destination_end].copy_from_slice(&source[input..literal_end]);
        input = literal_end;
        output = destination_end;
        if input == source.len() {
            break;
        }
        let offset = u16::from_le_bytes(source.get(input..input + 2)?.try_into().ok()?) as usize;
        input += 2;
        if offset == 0 || offset > output {
            return None;
        }
        let mut length = usize::from(token & 0x0F) + 4;
        if token & 0x0F == 15 {
            loop {
                let byte = *source.get(input)?;
                input += 1;
                length = length.checked_add(usize::from(byte))?;
                if byte != 255 {
                    break;
                }
            }
        }
        let match_end = output.checked_add(length)?;
        if match_end > out.len() {
            return None;
        }
        for index in 0..length {
            out[output + index] = out[output - offset + index];
        }
        output = match_end;
    }
    if output != out.len() {
        return None;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_zero_runs_and_patterns() {
        let mut page = [0_u8; BLOCK];
        assert!(compress(&page).is_some(), "全零页必须值得压缩");
        let compressed = compress(&page).unwrap();
        let mut restored = [0xFF_u8; BLOCK];
        decompress(&compressed, &mut restored).expect("解压全零页");
        assert_eq!(restored, [0_u8; BLOCK]);

        for (offset, byte) in page.iter_mut().enumerate() {
            *byte = (offset % 251) as u8;
        }
        let compressed = compress(&page).expect("周期页应可压缩");
        let mut restored = [0_u8; BLOCK];
        decompress(&compressed, &mut restored).expect("解压周期页");
        assert_eq!(restored, page);
    }

    #[test]
    fn rejects_corrupt_or_unprofitable_input() {
        let mut random = [0_u8; BLOCK];
        let mut state = 0x1234_5678_u32;
        for byte in &mut random {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (state >> 24) as u8;
        }
        assert!(compress(&random).is_none(), "随机页不应被压缩");
        assert!(decompress(&[0xFF, 0x00], &mut [0_u8; BLOCK]).is_none());
        assert!(decompress(&[0x00, 0x00, 0x00], &mut [0_u8; BLOCK]).is_none());
    }
}
