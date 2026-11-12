//! Native Task 检查点快照格式。
//!
//! 快照描述任务自身：主线程与附加线程的寄存器现场、父子关系、Region 元数据、
//! Arena 用量与用户页内容。设备、Provider 与跨任务共享能力不进入快照；格式带
//! 版本号，解析失败时返回 `None`。

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

/// 附加线程的现场与 TLS 基址。
///
/// 主线程现场保存在 [`Snapshot::context`]；本类型只描述同一进程内的其它线程，
/// 按槽位顺序排列。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadSnapshot {
    /// 平台寄存器现场编码。
    pub context: Vec<u8>,
    /// 线程指针（TLS 基址）。
    pub tls: u64,
}

/// Region 元数据；恢复时据此重建能力与映射描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionSnapshot {
    /// 用户虚拟地址。
    pub virt: u64,
    /// 能力内偏移。
    pub offset: u64,
    /// 映射长度。
    pub len: u64,
    /// Region 权利位。
    pub rights: u32,
    /// Region 标志位。
    pub flags: u32,
    /// 能力种类判别值。
    pub kind: u8,
    /// 能力总长度。
    pub capability_size: u64,
}

/// HMP 对象图的只读描述；不承载授权，恢复后由 Root/Memory Server 重新授予。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectSnapshot {
    /// 对象类别：0 Capacity，1 Frame，2 Capability，3 Region，4 其它。
    pub kind: u8,
    /// 句柄权利。
    pub rights: u32,
    /// 用户态句柄编号。
    pub number: u32,
    /// 持有该句柄的 TCB。
    pub owner: u64,
    /// 对象标识。
    pub object: u64,
    /// 对象大小（Capacity 配额 / Frame 长度 / 能力长度 / Region 长度）。
    pub size: u64,
    /// 物理起始地址；惰性对象为 0。
    pub phys: u64,
    /// 类别相关细节（Capacity 页大小 / Frame 页大小 / 能力种类 / Region 虚拟地址）。
    pub detail: u64,
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
    /// 附加线程。
    pub threads: Vec<ThreadSnapshot>,
    /// Region 元数据。
    pub regions: Vec<RegionSnapshot>,
    /// HMP 对象图描述；只记录，不用于重建授权。
    pub objects: Vec<ObjectSnapshot>,
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
            &self.threads,
            &self.regions,
            &self.objects,
        )
    }

    /// 返回编码后的字节数。
    pub fn encoded_size(&self) -> usize {
        encoded_size(
            self.name.len(),
            self.context.len(),
            &self.pages,
            &self.threads,
            &self.regions,
            &self.objects,
        )
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
        let count = cursor.u32()? as usize;
        if count > cursor.remaining() / 12 {
            return None;
        }
        let mut threads = Vec::with_capacity(count);
        for _ in 0..count {
            let context = cursor.bytes()?.to_vec();
            let tls = cursor.u64()?;
            threads.push(ThreadSnapshot { context, tls });
        }
        let count = cursor.u32()? as usize;
        if count > cursor.remaining() / 33 {
            return None;
        }
        let mut regions = Vec::with_capacity(count);
        for _ in 0..count {
            regions.push(RegionSnapshot {
                virt: cursor.u64()?,
                offset: cursor.u64()?,
                len: cursor.u64()?,
                rights: cursor.u32()?,
                flags: cursor.u32()?,
                kind: cursor.u8()?,
                capability_size: cursor.u64()?,
            });
        }
        let count = cursor.u32()? as usize;
        if count > cursor.remaining() / 49 {
            return None;
        }
        let mut objects = Vec::with_capacity(count);
        for _ in 0..count {
            objects.push(ObjectSnapshot {
                kind: cursor.u8()?,
                rights: cursor.u32()?,
                number: cursor.u32()?,
                owner: cursor.u64()?,
                object: cursor.u64()?,
                size: cursor.u64()?,
                phys: cursor.u64()?,
                detail: cursor.u64()?,
            });
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
            threads,
            regions,
            objects,
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
    threads: &[ThreadSnapshot],
    regions: &[RegionSnapshot],
    objects: &[ObjectSnapshot],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded_size(
        name.len(),
        context.len(),
        pages,
        threads,
        regions,
        objects,
    ));
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
    push_u32(&mut out, threads.len() as u32);
    for thread in threads {
        push_bytes(&mut out, &thread.context);
        push_u64(&mut out, thread.tls);
    }
    push_u32(&mut out, regions.len() as u32);
    for region in regions {
        push_u64(&mut out, region.virt);
        push_u64(&mut out, region.offset);
        push_u64(&mut out, region.len);
        push_u32(&mut out, region.rights);
        push_u32(&mut out, region.flags);
        out.push(region.kind);
        push_u64(&mut out, region.capability_size);
    }
    push_u32(&mut out, objects.len() as u32);
    for object in objects {
        out.push(object.kind);
        push_u32(&mut out, object.rights);
        push_u32(&mut out, object.number);
        push_u64(&mut out, object.owner);
        push_u64(&mut out, object.object);
        push_u64(&mut out, object.size);
        push_u64(&mut out, object.phys);
        push_u64(&mut out, object.detail);
    }
    out
}

fn encoded_size(
    name: usize,
    context: usize,
    pages: &[Page],
    threads: &[ThreadSnapshot],
    regions: &[RegionSnapshot],
    objects: &[ObjectSnapshot],
) -> usize {
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
    size += 4;
    for thread in threads {
        size += 4 + thread.context.len() + 8;
    }
    size += 4 + regions.len() * (8 + 8 + 8 + 4 + 4 + 1 + 8);
    size += 4 + objects.len() * (1 + 4 + 4 + 8 + 8 + 8 + 8 + 8);
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
            threads: vec![ThreadSnapshot {
                context: vec![5, 6, 7, 8],
                tls: 0x7000,
            }],
            regions: vec![RegionSnapshot {
                virt: 0x1000_0000,
                offset: 0,
                len: 0x1000,
                rights: 0x5,
                flags: 0x2,
                kind: 2,
                capability_size: 0x1000,
            }],
            objects: vec![ObjectSnapshot {
                kind: 2,
                rights: 0x1f,
                number: 0x3001,
                owner: 9,
                object: 0x10,
                size: 0x1000,
                phys: 0x40_0000,
                detail: 1,
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
        // 页计数位于页数据之前；页数据之后还有线程与 Region 两段。
        let threads_len = 4 + (4 + 4 + 8);
        let regions_len = 4 + (8 + 8 + 8 + 4 + 4 + 1 + 8);
        let objects_len = 4 + (1 + 4 + 4 + 8 + 8 + 8 + 8 + 8);
        let count_at = bytes.len() - 4096 - 20 - 4 - threads_len - regions_len - objects_len;
        bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Snapshot::decode(&bytes), None);
    }
}
