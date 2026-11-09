//! Native Task 检查点快照格式。
//!
//! 快照只描述任务自身：寄存器现场、父子关系、Arena 用量与用户页内容。设备与
//! 共享 Frame 不进入快照；格式带版本号，解析失败时返回 `None`。

#![no_std]
extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use hk_cadence::{CadenceSnapshot, MeterSnapshot};

/// 快照魔数。
pub const MAGIC: [u8; 4] = *b"HKCK";
/// 当前格式版本。
pub const VERSION: u32 = 1;

/// 快照中的共享页标记；不是硬件 PTE 位，恢复时由平台清除。
pub const PAGE_FLAG_SHARED: u64 = 1 << 63;

/// 一页用户内存。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// 用户虚拟地址。
    pub virt: u64,
    /// 原始页表属性位。
    pub flags: u64,
    /// 页内容；空表示整页为零。
    pub data: Vec<u8>,
}

/// 一次检查点快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// 原任务槽位；恢复必须回到同一槽位。
    pub slot: u32,
    /// 任务名。
    pub name: String,
    /// 平台寄存器现场编码。
    pub context: Vec<u8>,
    /// 父任务槽位。
    pub parent: Option<u64>,
    /// Arena 已用字节数。
    pub arena_used: u64,
    /// Cadence 契约快照。
    pub cadence: CadenceSnapshot,
    /// Meter 计量快照。
    pub meter: MeterSnapshot,
    /// 用户页。
    pub pages: Vec<Page>,
}

impl Snapshot {
    /// 编码为自描述字节流。
    pub fn encode(&self) -> Vec<u8> {
        encode_parts(
            self.slot,
            &self.name,
            &self.context,
            self.parent,
            self.arena_used,
            self.cadence,
            self.meter,
            &self.pages,
        )
    }

    /// 返回编码后的字节数。
    pub fn encoded_size(&self) -> usize {
        encoded_size(self.name.len(), self.context.len(), &self.pages)
    }

    /// 从字节流解码；格式不匹配或截断时返回 `None`。
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut cursor = Cursor { bytes, offset: 0 };
        if cursor.take(4)? != MAGIC {
            return None;
        }
        if cursor.u32()? != VERSION {
            return None;
        }
        let slot = cursor.u32()?;
        let name = String::from_utf8(cursor.bytes()?.to_vec()).ok()?;
        let context = cursor.bytes()?.to_vec();
        let parent = match cursor.u8()? {
            0 => None,
            1 => Some(cursor.u64()?),
            _ => return None,
        };
        let arena_used = cursor.u64()?;
        let cadence = CadenceSnapshot::decode(cursor.take(CadenceSnapshot::ENCODED_LEN)?).ok()?;
        let meter = MeterSnapshot::decode(cursor.take(MeterSnapshot::ENCODED_LEN)?).ok()?;
        let count = cursor.u32()? as usize;
        if count > cursor.remaining() / 20 {
            return None;
        }
        let mut pages = Vec::with_capacity(count);
        for _ in 0..count {
            let virt = cursor.u64()?;
            let flags = cursor.u64()?;
            let data = cursor.bytes()?.to_vec();
            pages.push(Page { virt, flags, data });
        }
        Some(Self {
            slot,
            name,
            context,
            parent,
            arena_used,
            cadence,
            meter,
            pages,
        })
    }
}

/// 直接编码各部分，避免构造拥有页列表的临时快照。
#[allow(clippy::too_many_arguments)]
pub fn encode_parts(
    slot: u32,
    name: &str,
    context: &[u8],
    parent: Option<u64>,
    arena_used: u64,
    cadence: CadenceSnapshot,
    meter: MeterSnapshot,
    pages: &[Page],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded_size(name.len(), context.len(), pages));
    out.extend_from_slice(&MAGIC);
    push_u32(&mut out, VERSION);
    push_u32(&mut out, slot);
    push_bytes(&mut out, name.as_bytes());
    push_bytes(&mut out, context);
    match parent {
        Some(parent) => {
            out.push(1);
            push_u64(&mut out, parent);
        }
        None => out.push(0),
    }
    push_u64(&mut out, arena_used);
    let mut cadence_bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    cadence.encode(&mut cadence_bytes);
    out.extend_from_slice(&cadence_bytes);
    let mut meter_bytes = [0_u8; MeterSnapshot::ENCODED_LEN];
    meter.encode(&mut meter_bytes);
    out.extend_from_slice(&meter_bytes);
    push_u32(&mut out, pages.len() as u32);
    for page in pages {
        push_u64(&mut out, page.virt);
        push_u64(&mut out, page.flags);
        push_bytes(&mut out, &page.data);
    }
    out
}

fn encoded_size(name: usize, context: usize, pages: &[Page]) -> usize {
    let mut size = 4
        + 4
        + 4
        + 4
        + name
        + 4
        + context
        + 1
        + 8
        + 8
        + CadenceSnapshot::ENCODED_LEN
        + MeterSnapshot::ENCODED_LEN
        + 4;
    for page in pages {
        size += 8 + 8 + 4 + page.data.len();
    }
    size
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(count)?;
        let slice = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let length = self.u32()? as usize;
        self.take(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sample() -> Snapshot {
        Snapshot {
            slot: 7,
            name: String::from("hk-server-echo"),
            context: vec![1, 2, 3, 4],
            parent: Some(2),
            arena_used: 0x1234,
            cadence: CadenceSnapshot {
                weight: 2048,
                epoch: 3,
                ..CadenceSnapshot::default()
            },
            meter: MeterSnapshot {
                consumed: 1234,
                virtual_pass: 5678,
                ..MeterSnapshot::default()
            },
            pages: vec![Page {
                virt: 0x1000_0000,
                flags: 0x27,
                data: vec![0xAA; 4096],
            }],
        }
    }

    #[test]
    fn round_trip() {
        let snapshot = sample();
        assert_eq!(Snapshot::decode(&snapshot.encode()), Some(snapshot));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample().encode();
        bytes[0] = b'X';
        assert_eq!(Snapshot::decode(&bytes), None);
    }

    #[test]
    fn rejects_truncated() {
        let bytes = sample().encode();
        assert_eq!(Snapshot::decode(&bytes[..bytes.len() - 1]), None);
    }

    #[test]
    fn rejects_huge_page_count() {
        let mut bytes = sample().encode();
        let count_at = bytes.len() - 4096 - 20 - 4;
        bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Snapshot::decode(&bytes), None);
    }
}
