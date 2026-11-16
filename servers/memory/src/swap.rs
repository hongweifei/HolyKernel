//! Memory Server 的页压缩格式：与内核交换层相同的 LZ4 block 变体。
//!
//! 服务端只负责保存/还原匿名页内容；页的驻留与回收仍由内核对象状态决定。

use alloc::vec::Vec;

/// 一页长度。
pub const BLOCK: usize = 4096;
const HASH_BITS: usize = 10;
const HASH_SIZE: usize = 1 << HASH_BITS;

fn hash(sequence: u32) -> usize {
    (sequence.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

fn emit_length(out: &mut Vec<u8>, mut length: usize) {
    while length >= 255 {
        out.push(255);
        length -= 255;
    }
    out.push(length as u8);
}

/// 压缩一页；没有收益时返回 `None`，调用方保存原始内容。
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
        let literals = position - anchor;
        out.push(((literals.min(15) as u8) << 4) | ((length - 4).min(15) as u8));
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
    }
    let literals = source.len() - anchor;
    out.push((literals.min(15) as u8) << 4);
    if literals >= 15 {
        emit_length(&mut out, literals - 15);
    }
    out.extend_from_slice(&source[anchor..]);
    (out.len() + 256 < source.len()).then_some(out)
}

/// 把压缩块还原到一页。
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
    (output == out.len()).then_some(())
}
