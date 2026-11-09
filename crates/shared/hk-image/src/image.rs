//! 镜像解析、校验与记录视图。

use crate::format::{
    CAPABILITY_LEN, DEPENDENCY_LEN, HEADER_SIZE, MAGIC, RECORD_CAPABILITY, RECORD_DEPENDENCY,
    RECORD_HEADER, RECORD_REGION, RECORD_SYMBOL, RECORD_TASK, REGION_LEN, SYMBOL_LEN, TASK_LEN,
    VERSION, align4, crc32c, read_u16, read_u32, read_u64,
};
use crate::types::{Capability, Dependency, Region, Symbol, Task};

/// 已解析的镜像。
#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    bytes: &'a [u8],
    records: u32,
    image_id: u64,
}

impl<'a> Image<'a> {
    /// 解析并校验镜像。
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < HEADER_SIZE || bytes[..4] != MAGIC {
            return None;
        }
        if read_u16(bytes, 4)? != VERSION || usize::from(read_u16(bytes, 6)?) != HEADER_SIZE {
            return None;
        }
        let records = read_u32(bytes, 8)?;
        let checksum = read_u32(bytes, 12)?;
        let image_id = read_u64(bytes, 16)?;
        let total_size = usize::try_from(read_u32(bytes, 24)?).ok()?;
        if total_size != bytes.len() || crc32c(&bytes[HEADER_SIZE..]) != checksum {
            return None;
        }
        let image = Self {
            bytes,
            records,
            image_id,
        };
        let mut tasks = 0;
        let mut count = 0;
        for (kind, _flags, payload) in image.records() {
            count += 1;
            if !validate(kind, payload) {
                return None;
            }
            if kind == RECORD_TASK {
                tasks += 1;
            }
        }
        if count != records || tasks != 1 {
            return None;
        }
        Some(image)
    }

    /// 返回内容标识。
    pub const fn image_id(&self) -> u64 {
        self.image_id
    }

    /// 返回 Task 记录。
    pub fn task(&self) -> Option<Task<'a>> {
        self.records()
            .find_map(|(kind, _, payload)| (kind == RECORD_TASK).then(|| task(payload)).flatten())
    }

    /// 遍历内存区域。
    pub fn regions(&self) -> impl Iterator<Item = Region<'a>> + 'a {
        self.records().filter_map(move |(kind, _, payload)| {
            (kind == RECORD_REGION).then(|| region(payload)).flatten()
        })
    }

    /// 遍历能力声明。
    pub fn capabilities(&self) -> impl Iterator<Item = Capability<'a>> + 'a {
        self.records().filter_map(move |(kind, _, payload)| {
            (kind == RECORD_CAPABILITY)
                .then(|| capability(payload))
                .flatten()
        })
    }

    /// 判断是否声明了指定能力。
    pub fn has_capability(&self, kind: u8) -> bool {
        self.capabilities()
            .any(|capability| capability.kind == kind)
    }

    /// 遍历调试符号。
    pub fn symbols(&self) -> impl Iterator<Item = Symbol<'a>> + 'a {
        self.records().filter_map(move |(kind, _, payload)| {
            (kind == RECORD_SYMBOL).then(|| symbol(payload)).flatten()
        })
    }

    /// 遍历服务依赖。
    pub fn dependencies(&self) -> impl Iterator<Item = Dependency<'a>> + 'a {
        self.records().filter_map(move |(kind, _, payload)| {
            (kind == RECORD_DEPENDENCY)
                .then(|| dependency(payload))
                .flatten()
        })
    }

    fn records(&self) -> impl Iterator<Item = (u16, u16, &'a [u8])> + 'a {
        let mut offset = HEADER_SIZE;
        let mut remaining = self.records;
        let bytes = self.bytes;
        core::iter::from_fn(move || {
            if remaining == 0 || offset + RECORD_HEADER > bytes.len() {
                return None;
            }
            let kind = read_u16(bytes, offset)?;
            let flags = read_u16(bytes, offset + 2)?;
            let length = usize::try_from(read_u32(bytes, offset + 4)?).ok()?;
            let start = offset + RECORD_HEADER;
            let end = start.checked_add(length)?;
            if end > bytes.len() {
                return None;
            }
            offset = align4(end);
            remaining -= 1;
            Some((kind, flags, &bytes[start..end]))
        })
    }
}

fn validate(kind: u16, payload: &[u8]) -> bool {
    match kind {
        RECORD_TASK => task(payload).is_some(),
        RECORD_REGION => region(payload).is_some(),
        RECORD_CAPABILITY => capability(payload).is_some(),
        RECORD_SYMBOL => symbol(payload).is_some(),
        RECORD_DEPENDENCY => dependency(payload).is_some(),
        _ => true,
    }
}

fn task(payload: &[u8]) -> Option<Task<'_>> {
    if payload.len() < TASK_LEN {
        return None;
    }
    let name_len = usize::from(read_u16(payload, 20)?);
    if payload.len() != TASK_LEN + name_len {
        return None;
    }
    let name = core::str::from_utf8(payload.get(TASK_LEN..TASK_LEN + name_len)?).ok()?;
    Some(Task {
        name,
        role: read_u32(payload, 0)?,
        entry: read_u64(payload, 4)?,
        stack_size: read_u32(payload, 12)?,
        arena_size: read_u32(payload, 16)?,
    })
}

fn region(payload: &[u8]) -> Option<Region<'_>> {
    if payload.len() < REGION_LEN {
        return None;
    }
    let data_size = usize::try_from(read_u32(payload, 20)?).ok()?;
    if payload.len() != REGION_LEN + data_size {
        return None;
    }
    let file = payload.get(REGION_LEN..REGION_LEN + data_size)?;
    let memory_size = read_u64(payload, 8)?;
    if memory_size < data_size as u64 {
        return None;
    }
    Some(Region {
        virtual_address: read_u64(payload, 0)?,
        memory_size,
        flags: read_u32(payload, 16)?,
        file,
    })
}

fn capability(payload: &[u8]) -> Option<Capability<'_>> {
    if payload.len() < CAPABILITY_LEN {
        return None;
    }
    let name_len = usize::from(read_u16(payload, 5)?);
    if payload.len() != CAPABILITY_LEN + name_len {
        return None;
    }
    let name =
        core::str::from_utf8(payload.get(CAPABILITY_LEN..CAPABILITY_LEN + name_len)?).ok()?;
    Some(Capability {
        kind: payload[0],
        rights: read_u32(payload, 1)?,
        name,
    })
}

fn symbol(payload: &[u8]) -> Option<Symbol<'_>> {
    if payload.len() < SYMBOL_LEN {
        return None;
    }
    let name_len = usize::from(read_u16(payload, 8)?);
    if payload.len() != SYMBOL_LEN + name_len {
        return None;
    }
    let name = core::str::from_utf8(payload.get(SYMBOL_LEN..SYMBOL_LEN + name_len)?).ok()?;
    Some(Symbol {
        address: read_u64(payload, 0)?,
        name,
    })
}

fn dependency(payload: &[u8]) -> Option<Dependency<'_>> {
    if payload.len() < DEPENDENCY_LEN {
        return None;
    }
    let name_len = usize::from(read_u16(payload, 2)?);
    if payload.len() != DEPENDENCY_LEN + name_len {
        return None;
    }
    let name =
        core::str::from_utf8(payload.get(DEPENDENCY_LEN..DEPENDENCY_LEN + name_len)?).ok()?;
    Some(Dependency {
        name,
        min_version: read_u16(payload, 0)?,
    })
}
