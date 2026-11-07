//! HolyFS 对象记录与磁盘编码。

pub use hk_fs::{DocumentInfo, DocumentKind, EntryInfo, RevisionFlags, RevisionInfo, SnapshotInfo};

use alloc::vec::Vec;

use crate::crc32c::crc32c;
use crate::error::HolyError;
use crate::ids::{DocId, RevisionId};

/// 单个修订可保存的最大 extent 数。
pub const MAX_EXTENTS: usize = 16;
/// 名字的最大字节数。
pub const MAX_NAME_LEN: usize = 255;
/// 文档记录编码长度。
pub const DOCUMENT_RECORD_SIZE: usize = 64;
/// 修订记录编码长度。
pub const REVISION_RECORD_SIZE: usize = 272;
/// 集合项编码长度。
pub const ENTRY_RECORD_SIZE: usize = 24;

/// 文档当前修订的原始存储载荷。
pub struct StoredContent {
    /// 原始载荷；压缩文档保存压缩后的字节。
    pub payload: Vec<u8>,
    /// 解压后的内容长度。
    pub size: usize,
    /// 载荷是否压缩。
    pub compressed: bool,
}

/// 卷配额；0 表示不限。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Quota {
    /// 文档数。
    pub documents: u64,
    /// 逻辑内容字节数。
    pub bytes: u64,
    /// 修订数。
    pub revisions: u64,
    /// 快照数。
    pub snapshots: u64,
    /// 标签数。
    pub tags: u64,
}

/// 维护扫描结果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// 文档数。
    pub documents: u64,
    /// 修订数。
    pub revisions: u64,
    /// 集合修订数。
    pub collections: u64,
    /// 集合项数。
    pub entries: u64,
    /// 标签数。
    pub tags: u64,
    /// 快照数。
    pub snapshots: u64,
    /// 内容总字节数。
    pub content_bytes: u64,
    /// 已分配但不可达的块数。
    pub leaked_blocks: u64,
    /// 本次修复的块数。
    pub repaired_blocks: u64,
}

/// 修订 payload 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    /// 内容 extent。
    Content,
    /// 集合项索引。
    Collection,
    /// 内容与集合项同时存在。
    Both,
}

impl PayloadKind {
    fn encode(self) -> u8 {
        match self {
            Self::Content => 0,
            Self::Collection => 1,
            Self::Both => 2,
        }
    }

    fn decode(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Content),
            1 => Some(Self::Collection),
            2 => Some(Self::Both),
            _ => None,
        }
    }
}

/// 内容区间；以 4 KiB 块为单位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// 起始块号。
    pub start: u64,
    /// 块数量。
    pub blocks: u32,
}

impl Extent {
    /// 区间结束块号（不含）。
    pub const fn end(self) -> u64 {
        self.start + self.blocks as u64
    }

    /// 区间字节长度。
    pub const fn bytes(self) -> u64 {
        self.blocks as u64 * crate::BLOCK_SIZE as u64
    }
}

/// 文档记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentRecord {
    /// 文档身份。
    pub document: DocId,
    /// 当前 head 修订。
    pub head: RevisionId,
    /// 文档类型。
    pub kind: DocumentKind,
}

impl DocumentRecord {
    /// 编码为固定长度记录。
    pub fn encode(self) -> [u8; DOCUMENT_RECORD_SIZE] {
        let mut bytes = [0_u8; DOCUMENT_RECORD_SIZE];
        bytes[0..16].copy_from_slice(&self.document.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.head.to_le_bytes());
        bytes[24] = match self.kind {
            DocumentKind::Content => 0,
            DocumentKind::Collection => 1,
        };
        seal(&mut bytes);
        bytes
    }

    /// 从固定长度记录解码。
    pub fn decode(bytes: &[u8]) -> Result<Self, HolyError> {
        if bytes.len() != DOCUMENT_RECORD_SIZE || !verify(bytes) {
            return Err(HolyError::Corrupt);
        }
        let document =
            DocId::from_le_bytes(bytes[0..16].try_into().map_err(|_| HolyError::Corrupt)?);
        let head =
            RevisionId::from_le_bytes(bytes[16..24].try_into().map_err(|_| HolyError::Corrupt)?);
        let kind = match bytes[24] {
            0 => DocumentKind::Content,
            1 => DocumentKind::Collection,
            _ => return Err(HolyError::Corrupt),
        };
        Ok(Self {
            document,
            head,
            kind,
        })
    }
}

/// 修订记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionRecord {
    /// 修订身份。
    pub id: RevisionId,
    /// 所属文档。
    pub document: DocId,
    /// 父修订；根修订为 `None`。
    pub parent: Option<RevisionId>,
    /// payload 类型。
    pub payload: PayloadKind,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间；0 表示时钟尚未写入。
    pub moment: u64,
    /// 修订标志。
    pub flags: RevisionFlags,
    /// 明文内容哈希；内容修订写入时计算。
    pub content_hash: u64,
    /// extent 数量。
    pub extent_count: u16,
    /// 内容 extent。
    pub extents: [Extent; MAX_EXTENTS],
    /// 集合项索引树根。
    pub entries_root: u64,
}

impl RevisionRecord {
    /// 空内容修订。
    pub const fn empty(id: RevisionId, document: DocId, parent: Option<RevisionId>) -> Self {
        Self {
            id,
            document,
            parent,
            payload: PayloadKind::Content,
            size: 0,
            moment: 0,
            flags: RevisionFlags::NONE,
            content_hash: 0,
            extent_count: 0,
            extents: [Extent {
                start: 0,
                blocks: 0,
            }; MAX_EXTENTS],
            entries_root: 0,
        }
    }

    /// 返回有效 extent。
    pub fn extents(&self) -> &[Extent] {
        &self.extents[..self.extent_count as usize]
    }

    /// 判断修订是否承载集合项。
    pub const fn payload_has_collection(&self) -> bool {
        matches!(self.payload, PayloadKind::Collection | PayloadKind::Both)
    }

    /// 编码为固定长度记录。
    pub fn encode(&self) -> [u8; REVISION_RECORD_SIZE] {
        let mut bytes = [0_u8; REVISION_RECORD_SIZE];
        bytes[0..8].copy_from_slice(&self.id.to_le_bytes());
        bytes[8..24].copy_from_slice(&self.document.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.parent.map_or(0, RevisionId::get).to_le_bytes());
        bytes[32] = self.payload.encode();
        bytes[36..44].copy_from_slice(&self.size.to_le_bytes());
        bytes[44..52].copy_from_slice(&self.moment.to_le_bytes());
        bytes[52..56].copy_from_slice(&self.flags.bits().to_le_bytes());
        bytes[56..64].copy_from_slice(&self.content_hash.to_le_bytes());
        bytes[64..66].copy_from_slice(&self.extent_count.to_le_bytes());
        for (index, extent) in self.extents().iter().enumerate() {
            let offset = 68 + index * 12;
            bytes[offset..offset + 8].copy_from_slice(&extent.start.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&extent.blocks.to_le_bytes());
        }
        bytes[260..268].copy_from_slice(&self.entries_root.to_le_bytes());
        seal(&mut bytes);
        bytes
    }

    /// 从固定长度记录解码。
    pub fn decode(bytes: &[u8]) -> Result<Self, HolyError> {
        if bytes.len() != REVISION_RECORD_SIZE || !verify(bytes) {
            return Err(HolyError::Corrupt);
        }
        let id = RevisionId::from_le_bytes(bytes[0..8].try_into().map_err(|_| HolyError::Corrupt)?);
        let document =
            DocId::from_le_bytes(bytes[8..24].try_into().map_err(|_| HolyError::Corrupt)?);
        let parent = u64::from_le_bytes(bytes[24..32].try_into().map_err(|_| HolyError::Corrupt)?);
        let payload = PayloadKind::decode(bytes[32]).ok_or(HolyError::Corrupt)?;
        let size = u64::from_le_bytes(bytes[36..44].try_into().map_err(|_| HolyError::Corrupt)?);
        let moment = u64::from_le_bytes(bytes[44..52].try_into().map_err(|_| HolyError::Corrupt)?);
        let flags = RevisionFlags::from_bits(u32::from_le_bytes(
            bytes[52..56].try_into().map_err(|_| HolyError::Corrupt)?,
        ));
        let content_hash =
            u64::from_le_bytes(bytes[56..64].try_into().map_err(|_| HolyError::Corrupt)?);
        let extent_count =
            u16::from_le_bytes(bytes[64..66].try_into().map_err(|_| HolyError::Corrupt)?);
        if extent_count as usize > MAX_EXTENTS {
            return Err(HolyError::Corrupt);
        }
        let mut extents = [Extent {
            start: 0,
            blocks: 0,
        }; MAX_EXTENTS];
        for (index, extent) in extents.iter_mut().enumerate().take(extent_count as usize) {
            let offset = 68 + index * 12;
            extent.start = u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .map_err(|_| HolyError::Corrupt)?,
            );
            extent.blocks = u32::from_le_bytes(
                bytes[offset + 8..offset + 12]
                    .try_into()
                    .map_err(|_| HolyError::Corrupt)?,
            );
        }
        let entries_root =
            u64::from_le_bytes(bytes[260..268].try_into().map_err(|_| HolyError::Corrupt)?);
        Ok(Self {
            id,
            document,
            parent: (parent != 0).then(|| RevisionId::new(parent)),
            payload,
            size,
            moment,
            flags,
            content_hash,
            extent_count,
            extents,
            entries_root,
        })
    }
}

/// 集合项记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryRecord {
    /// 目标文档。
    pub document: DocId,
    /// 文档类型。
    pub kind: DocumentKind,
    /// 集合项标志；保留给可见性与封存。
    pub flags: u8,
}

impl EntryRecord {
    /// 编码为固定长度记录。
    pub fn encode(self) -> [u8; ENTRY_RECORD_SIZE] {
        let mut bytes = [0_u8; ENTRY_RECORD_SIZE];
        bytes[0..16].copy_from_slice(&self.document.to_le_bytes());
        bytes[16] = match self.kind {
            DocumentKind::Content => 0,
            DocumentKind::Collection => 1,
        };
        bytes[17] = self.flags;
        seal(&mut bytes);
        bytes
    }

    /// 从固定长度记录解码。
    pub fn decode(bytes: &[u8]) -> Result<Self, HolyError> {
        if bytes.len() != ENTRY_RECORD_SIZE || !verify(bytes) {
            return Err(HolyError::Corrupt);
        }
        let document =
            DocId::from_le_bytes(bytes[0..16].try_into().map_err(|_| HolyError::Corrupt)?);
        let kind = match bytes[16] {
            0 => DocumentKind::Content,
            1 => DocumentKind::Collection,
            _ => return Err(HolyError::Corrupt),
        };
        Ok(Self {
            document,
            kind,
            flags: bytes[17],
        })
    }
}

fn seal(bytes: &mut [u8]) {
    let checksum_offset = bytes.len() - 4;
    bytes[checksum_offset..].fill(0);
    let checksum = crc32c(bytes);
    bytes[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
}

fn verify(bytes: &[u8]) -> bool {
    let checksum_offset = bytes.len() - 4;
    let stored = u32::from_le_bytes(bytes[checksum_offset..].try_into().unwrap_or_default());
    let mut copy = [0_u8; REVISION_RECORD_SIZE];
    if bytes.len() > copy.len() {
        return false;
    }
    copy[..bytes.len()].copy_from_slice(bytes);
    copy[checksum_offset..checksum_offset + 4].fill(0);
    crc32c(&copy[..bytes.len()]) == stored
}

/// 校验名字是否合法。
pub fn validate_name(name: &str) -> Result<(), HolyError> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_NAME_LEN
        || name.contains('/')
        || name == "."
        || name == ".."
    {
        return Err(HolyError::Invalid);
    }
    Ok(())
}
