//! Superblock 编码、校验与卷布局常量。

use crate::crc32c::crc32c;
use crate::error::HolyError;
use crate::ids::{DocId, RevisionId, VolumeId};

/// 文件系统块大小。
pub const BLOCK_SIZE: usize = 4096;
/// 超级块 A 的块号。
pub const SUPERBLOCK_A: u64 = 0;
/// 超级块 B 的块号。
pub const SUPERBLOCK_B: u64 = 1;
/// 块位图起始块号。
pub const BITMAP_START: u64 = 2;
/// 超级块有效字节数。
pub const SUPERBLOCK_SIZE: usize = 160;
/// 文件系统魔数。
pub const MAGIC: u64 = 0x484F_4C59_4653_3031;
/// 格式版本。
pub const VERSION: u32 = 1;
/// 默认卷身份。
pub const DEFAULT_VOLUME_ID: VolumeId = VolumeId::new(0x484F_4C59_4653_0001);

/// 卷提交超级块。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Superblock {
    /// 提交代数。
    pub generation: u64,
    /// 卷身份。
    pub volume_id: VolumeId,
    /// 卷总块数。
    pub total_blocks: u64,
    /// 块位图起始块号。
    pub bitmap_start: u64,
    /// 块位图占用块数。
    pub bitmap_blocks: u32,
    /// 根文档。
    pub root_document: DocId,
    /// 文档表树根。
    pub document_tree: u64,
    /// 修订表树根。
    pub revision_tree: u64,
    /// 标签表树根；0 表示旧卷没有标签。
    pub tag_tree: u64,
    /// 快照表树根；0 表示旧卷没有快照。
    pub snapshot_tree: u64,
    /// 下一个快照编号。
    pub next_snapshot: u64,
    /// 下一个标签编号。
    pub next_tag: u32,
    /// 下一个文档身份。
    pub next_document: DocId,
    /// 下一个修订身份。
    pub next_revision: u64,
}

impl Superblock {
    /// 构造空卷超级块。
    pub const fn empty(total_blocks: u64, bitmap_blocks: u32) -> Self {
        Self {
            generation: 0,
            volume_id: DEFAULT_VOLUME_ID,
            total_blocks,
            bitmap_start: BITMAP_START,
            bitmap_blocks,
            root_document: DocId::ROOT,
            document_tree: 0,
            revision_tree: 0,
            tag_tree: 0,
            snapshot_tree: 0,
            next_snapshot: 1,
            next_tag: 1,
            next_document: DocId::new(DocId::ROOT.get() + 1),
            next_revision: RevisionId::ROOT.get() + 1,
        }
    }

    /// 编码为固定长度超级块。
    pub fn encode(self) -> [u8; SUPERBLOCK_SIZE] {
        let mut bytes = [0_u8; SUPERBLOCK_SIZE];
        bytes[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.tag_tree.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.generation.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.total_blocks.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.bitmap_start.to_le_bytes());
        bytes[48..52].copy_from_slice(&self.bitmap_blocks.to_le_bytes());
        bytes[52..56].copy_from_slice(&self.next_tag.to_le_bytes());
        bytes[56..72].copy_from_slice(&self.volume_id.to_le_bytes());
        bytes[72..88].copy_from_slice(&self.root_document.to_le_bytes());
        bytes[88..96].copy_from_slice(&self.document_tree.to_le_bytes());
        bytes[96..104].copy_from_slice(&self.revision_tree.to_le_bytes());
        bytes[104..120].copy_from_slice(&self.next_document.to_le_bytes());
        bytes[120..128].copy_from_slice(&self.next_revision.to_le_bytes());
        bytes[132..140].copy_from_slice(&self.snapshot_tree.to_le_bytes());
        bytes[140..148].copy_from_slice(&self.next_snapshot.to_le_bytes());
        let checksum = crc32c(&bytes[..128]);
        bytes[128..132].copy_from_slice(&checksum.to_le_bytes());
        let snapshot_checksum = crc32c(&bytes[132..148]);
        bytes[148..152].copy_from_slice(&snapshot_checksum.to_le_bytes());
        bytes
    }

    /// 解码并校验超级块。
    pub fn decode(bytes: &[u8]) -> Result<Self, HolyError> {
        if bytes.len() < SUPERBLOCK_SIZE {
            return Err(HolyError::Corrupt);
        }
        let magic = u64::from_le_bytes(bytes[0..8].try_into().map_err(|_| HolyError::Corrupt)?);
        let version = u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| HolyError::Corrupt)?);
        let block_size =
            u32::from_le_bytes(bytes[12..16].try_into().map_err(|_| HolyError::Corrupt)?);
        let stored =
            u32::from_le_bytes(bytes[128..132].try_into().map_err(|_| HolyError::Corrupt)?);
        if magic != MAGIC || version != VERSION || block_size as usize != BLOCK_SIZE {
            return Err(HolyError::Corrupt);
        }
        if crc32c(&bytes[..128]) != stored {
            return Err(HolyError::Corrupt);
        }
        let tag_tree =
            u64::from_le_bytes(bytes[16..24].try_into().map_err(|_| HolyError::Corrupt)?);
        let generation =
            u64::from_le_bytes(bytes[24..32].try_into().map_err(|_| HolyError::Corrupt)?);
        let total_blocks =
            u64::from_le_bytes(bytes[32..40].try_into().map_err(|_| HolyError::Corrupt)?);
        let bitmap_start =
            u64::from_le_bytes(bytes[40..48].try_into().map_err(|_| HolyError::Corrupt)?);
        let bitmap_blocks =
            u32::from_le_bytes(bytes[48..52].try_into().map_err(|_| HolyError::Corrupt)?);
        let volume_id =
            VolumeId::from_le_bytes(bytes[56..72].try_into().map_err(|_| HolyError::Corrupt)?);
        let root_document =
            DocId::from_le_bytes(bytes[72..88].try_into().map_err(|_| HolyError::Corrupt)?);
        let document_tree =
            u64::from_le_bytes(bytes[88..96].try_into().map_err(|_| HolyError::Corrupt)?);
        let revision_tree =
            u64::from_le_bytes(bytes[96..104].try_into().map_err(|_| HolyError::Corrupt)?);
        let next_tag =
            u32::from_le_bytes(bytes[52..56].try_into().map_err(|_| HolyError::Corrupt)?).max(1);
        let next_document =
            DocId::from_le_bytes(bytes[104..120].try_into().map_err(|_| HolyError::Corrupt)?);
        let next_revision =
            u64::from_le_bytes(bytes[120..128].try_into().map_err(|_| HolyError::Corrupt)?);
        let snapshot_tree =
            u64::from_le_bytes(bytes[132..140].try_into().map_err(|_| HolyError::Corrupt)?);
        let next_snapshot =
            u64::from_le_bytes(bytes[140..148].try_into().map_err(|_| HolyError::Corrupt)?).max(1);
        let snapshot_checksum =
            u32::from_le_bytes(bytes[148..152].try_into().map_err(|_| HolyError::Corrupt)?);
        if (snapshot_tree != 0 || next_snapshot != 1)
            && crc32c(&bytes[132..148]) != snapshot_checksum
        {
            return Err(HolyError::Corrupt);
        }
        let bitmap_end = bitmap_start
            .checked_add(u64::from(bitmap_blocks))
            .ok_or(HolyError::Corrupt)?;
        if total_blocks == 0
            || bitmap_blocks == 0
            || bitmap_end > total_blocks
            || document_tree >= total_blocks
            || revision_tree >= total_blocks
            || (tag_tree != 0 && tag_tree >= total_blocks)
            || (snapshot_tree != 0 && snapshot_tree >= total_blocks)
        {
            return Err(HolyError::Corrupt);
        }
        Ok(Self {
            generation,
            volume_id,
            total_blocks,
            bitmap_start,
            bitmap_blocks,
            root_document,
            document_tree,
            revision_tree,
            tag_tree,
            snapshot_tree,
            next_snapshot,
            next_tag,
            next_document,
            next_revision,
        })
    }
}
