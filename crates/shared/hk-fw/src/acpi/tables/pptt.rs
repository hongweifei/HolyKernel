use alloc::vec::Vec;

use super::{HEADER, SIGNATURE_PPTT, table_length, u8_at, u32_at};

/// PPTT 处理器拓扑。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorTopology {
    /// 条目标志。
    pub flags: u32,
    /// 父节点索引；0 表示根。
    pub parent: u32,
    /// ACPI 处理器编号。
    pub processor_id: u32,
    /// 物理封装编号。
    pub package_id: u32,
}

/// PPTT 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pptt {
    /// 处理器条目。
    pub processors: Vec<ProcessorTopology>,
}

/// 解析 PPTT。
pub fn parse_pptt(bytes: &[u8]) -> Option<Pptt> {
    let length = table_length(bytes, SIGNATURE_PPTT)?;
    let bytes = &bytes[..length];
    if bytes.len() < HEADER {
        return None;
    }
    let mut pptt = Pptt::default();
    let mut offset = HEADER;
    while offset + 2 <= bytes.len() {
        let entry_type = u8_at(bytes, offset)?;
        let entry_len = u8_at(bytes, offset + 1)? as usize;
        if entry_len < 2 || offset + entry_len > bytes.len() {
            break;
        }
        let entry = &bytes[offset..offset + entry_len];
        if entry_type == 0 && entry_len >= 20 {
            pptt.processors.push(ProcessorTopology {
                flags: u32_at(entry, 4)?,
                parent: u32_at(entry, 8)?,
                processor_id: u32_at(entry, 12)?,
                package_id: u32_at(entry, 16)?,
            });
        }
        offset += entry_len;
    }
    Some(pptt)
}
