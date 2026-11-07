//! 集合项、文档视图、修订、标签与标量的编解码。

use super::ids::{DocId, DocumentKind, RevisionId};
use super::wire::{
    DOC_LEN, DOCUMENT_LEN, DRIVE_LEN, ENTRY_HEADER, KIND_COLLECTION, KIND_CONTENT, MAINTENANCE_LEN,
    QUOTA_LEN, REVISION_ID_LEN, REVISION_LEN, SNAPSHOT_HEADER, TAG_HEADER, TOKEN_LEN, VOLUME_LEN,
};

/// 集合项视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// 文档类型。
    pub kind: u8,
    /// 文档身份。
    pub document: u128,
    /// 集合内名字。
    pub name: &'a str,
}

/// 文档视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentView {
    /// 文档类型。
    pub kind: u8,
    /// head 修订。
    pub head: u64,
    /// head 的父修订；0 表示根。
    pub parent: u64,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间。
    pub moment: u64,
}

/// 卷视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeView {
    /// 卷身份。
    pub volume: u128,
    /// 提交代数。
    pub generation: u64,
    /// 卷代；恢复推进。
    pub epoch: u64,
    /// 根文档。
    pub root: DocId,
}

/// 编码卷视图。
pub fn encode_volume(out: &mut [u8], view: VolumeView) -> Option<usize> {
    if out.len() < VOLUME_LEN {
        return None;
    }
    out[0..16].copy_from_slice(&view.volume.to_le_bytes());
    out[16..24].copy_from_slice(&view.generation.to_le_bytes());
    out[24..32].copy_from_slice(&view.epoch.to_le_bytes());
    out[32..48].copy_from_slice(&view.root.to_le_bytes());
    Some(VOLUME_LEN)
}

/// 维护报告。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaintenanceView {
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
    /// 泄漏块数。
    pub leaked_blocks: u64,
    /// 修复块数。
    pub repaired_blocks: u64,
}

/// 编码维护报告。
pub fn encode_maintenance(out: &mut [u8], view: MaintenanceView) -> Option<usize> {
    if out.len() < MAINTENANCE_LEN {
        return None;
    }
    let values = [
        view.documents,
        view.revisions,
        view.collections,
        view.entries,
        view.tags,
        view.snapshots,
        view.content_bytes,
        view.leaked_blocks,
        view.repaired_blocks,
    ];
    for (index, value) in values.iter().enumerate() {
        let offset = index * 8;
        out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    Some(MAINTENANCE_LEN)
}

/// 解码维护报告。
pub fn decode_maintenance(bytes: &[u8]) -> Option<MaintenanceView> {
    if bytes.len() != MAINTENANCE_LEN {
        return None;
    }
    let mut values = [0_u64; 9];
    for (index, value) in values.iter_mut().enumerate() {
        let offset = index * 8;
        *value = u64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
    }
    Some(MaintenanceView {
        documents: values[0],
        revisions: values[1],
        collections: values[2],
        entries: values[3],
        tags: values[4],
        snapshots: values[5],
        content_bytes: values[6],
        leaked_blocks: values[7],
        repaired_blocks: values[8],
    })
}

#[cfg(test)]
mod tests {
    use super::{
        MaintenanceView, QuotaView, decode_maintenance, decode_quota, encode_maintenance,
        encode_quota,
    };

    #[test]
    fn round_trips_maintenance_report() {
        let view = MaintenanceView {
            documents: 2,
            revisions: 3,
            collections: 1,
            entries: 4,
            tags: 5,
            snapshots: 6,
            content_bytes: 7,
            leaked_blocks: 8,
            repaired_blocks: 9,
        };
        let mut bytes = [0_u8; 72];
        assert_eq!(encode_maintenance(&mut bytes, view), Some(72));
        assert_eq!(decode_maintenance(&bytes), Some(view));
    }

    #[test]
    fn round_trips_quota() {
        let quota = QuotaView {
            documents: 1,
            bytes: 2,
            revisions: 3,
            snapshots: 4,
            tags: 5,
        };
        let mut bytes = [0_u8; 40];
        assert_eq!(encode_quota(&mut bytes, quota), Some(40));
        assert_eq!(decode_quota(&bytes), Some(quota));
    }
}

/// 配额视图；0 表示不限。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuotaView {
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

/// 编码配额。
pub fn encode_quota(out: &mut [u8], quota: QuotaView) -> Option<usize> {
    if out.len() < QUOTA_LEN {
        return None;
    }
    let values = [
        quota.documents,
        quota.bytes,
        quota.revisions,
        quota.snapshots,
        quota.tags,
    ];
    for (index, value) in values.iter().enumerate() {
        let offset = index * 8;
        out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    Some(QUOTA_LEN)
}

/// 解码配额。
pub fn decode_quota(bytes: &[u8]) -> Option<QuotaView> {
    if bytes.len() != QUOTA_LEN {
        return None;
    }
    let mut values = [0_u64; 5];
    for (index, value) in values.iter_mut().enumerate() {
        let offset = index * 8;
        *value = u64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
    }
    Some(QuotaView {
        documents: values[0],
        bytes: values[1],
        revisions: values[2],
        snapshots: values[3],
        tags: values[4],
    })
}

/// 盘符视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriveView {
    /// 盘符。
    pub letter: u8,
    /// 卷视图。
    pub volume: VolumeView,
}

/// 编码盘符视图。
pub fn encode_drive(out: &mut [u8], offset: usize, view: DriveView) -> Option<usize> {
    let end = offset.checked_add(DRIVE_LEN)?;
    if end > out.len() {
        return None;
    }
    out[offset] = view.letter;
    encode_volume(&mut out[offset + 1..end], view.volume)?;
    Some(end)
}

/// 盘符视图迭代器。
pub struct Drives<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for Drives<'_> {
    type Item = DriveView;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + DRIVE_LEN > self.bytes.len() {
            return None;
        }
        let letter = self.bytes[start];
        let volume = decode_volume(&self.bytes[start + 1..start + DRIVE_LEN])?;
        self.offset = start + DRIVE_LEN;
        Some(DriveView { letter, volume })
    }
}

/// 解析盘符序列。
pub fn decode_drives(bytes: &[u8]) -> Drives<'_> {
    Drives { bytes, offset: 0 }
}

/// 解析卷视图。
pub fn decode_volume(bytes: &[u8]) -> Option<VolumeView> {
    if bytes.len() < VOLUME_LEN {
        return None;
    }
    Some(VolumeView {
        volume: u128::from_le_bytes(bytes[0..16].try_into().ok()?),
        generation: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        epoch: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        root: DocId::from_le_bytes(bytes[32..48].try_into().ok()?),
    })
}

/// 修订视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionView {
    /// 修订身份。
    pub id: u64,
    /// 父修订；0 表示根。
    pub parent: u64,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间。
    pub moment: u64,
    /// 修订标志。
    pub flags: u32,
}

/// 编码一个集合项，返回写入结束位置。
pub fn encode_entry(
    out: &mut [u8],
    offset: usize,
    kind: u8,
    document: u128,
    name: &str,
) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() > u8::MAX as usize {
        return None;
    }
    let end = offset.checked_add(ENTRY_HEADER)?.checked_add(bytes.len())?;
    if end > out.len() {
        return None;
    }
    out[offset] = kind;
    out[offset + 1..offset + 17].copy_from_slice(&document.to_le_bytes());
    out[offset + 17] = bytes.len() as u8;
    out[offset + 18..end].copy_from_slice(bytes);
    Some(end)
}

/// 集合项迭代器。
pub struct Entries<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + ENTRY_HEADER > self.bytes.len() {
            return None;
        }
        let kind = self.bytes[start];
        let document = u128::from_le_bytes(self.bytes[start + 1..start + 17].try_into().ok()?);
        let length = self.bytes[start + 17] as usize;
        let name_start = start + ENTRY_HEADER;
        let end = name_start.checked_add(length)?;
        if end > self.bytes.len() {
            return None;
        }
        let name = core::str::from_utf8(&self.bytes[name_start..end]).ok()?;
        self.offset = end;
        Some(Entry {
            kind,
            document,
            name,
        })
    }
}

/// 解析集合项序列。
pub fn decode_entries(bytes: &[u8]) -> Entries<'_> {
    Entries { bytes, offset: 0 }
}

/// 解析文档视图。
pub fn decode_document(bytes: &[u8]) -> Option<DocumentView> {
    if bytes.len() < DOCUMENT_LEN {
        return None;
    }
    Some(DocumentView {
        kind: bytes[0],
        head: u64::from_le_bytes(bytes[1..9].try_into().ok()?),
        parent: u64::from_le_bytes(bytes[9..17].try_into().ok()?),
        size: u64::from_le_bytes(bytes[17..25].try_into().ok()?),
        moment: u64::from_le_bytes(bytes[25..33].try_into().ok()?),
    })
}

/// 编码文档视图。
pub fn encode_document(out: &mut [u8], view: DocumentView) -> Option<usize> {
    if out.len() < DOCUMENT_LEN {
        return None;
    }
    out[0] = view.kind;
    out[1..9].copy_from_slice(&view.head.to_le_bytes());
    out[9..17].copy_from_slice(&view.parent.to_le_bytes());
    out[17..25].copy_from_slice(&view.size.to_le_bytes());
    out[25..33].copy_from_slice(&view.moment.to_le_bytes());
    Some(DOCUMENT_LEN)
}

/// 在 `offset` 处编码一条修订视图，返回写入结束位置。
pub fn encode_revision(out: &mut [u8], offset: usize, view: RevisionView) -> Option<usize> {
    let end = offset.checked_add(REVISION_LEN)?;
    if end > out.len() {
        return None;
    }
    out[offset..offset + 8].copy_from_slice(&view.id.to_le_bytes());
    out[offset + 8..offset + 16].copy_from_slice(&view.parent.to_le_bytes());
    out[offset + 16..offset + 24].copy_from_slice(&view.size.to_le_bytes());
    out[offset + 24..offset + 32].copy_from_slice(&view.moment.to_le_bytes());
    out[offset + 32..offset + 36].copy_from_slice(&view.flags.to_le_bytes());
    Some(end)
}

/// 修订视图迭代器。
pub struct Revisions<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for Revisions<'_> {
    type Item = RevisionView;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + REVISION_LEN > self.bytes.len() {
            return None;
        }
        let view = RevisionView {
            id: u64::from_le_bytes(self.bytes[start..start + 8].try_into().ok()?),
            parent: u64::from_le_bytes(self.bytes[start + 8..start + 16].try_into().ok()?),
            size: u64::from_le_bytes(self.bytes[start + 16..start + 24].try_into().ok()?),
            moment: u64::from_le_bytes(self.bytes[start + 24..start + 32].try_into().ok()?),
            flags: u32::from_le_bytes(self.bytes[start + 32..start + 36].try_into().ok()?),
        };
        self.offset = start + REVISION_LEN;
        Some(view)
    }
}

/// 解析修订序列。
pub fn decode_revisions(bytes: &[u8]) -> Revisions<'_> {
    Revisions { bytes, offset: 0 }
}

/// 解析文档身份。
pub fn decode_doc_id(bytes: &[u8]) -> Option<DocId> {
    Some(DocId::from_le_bytes(bytes.get(..DOC_LEN)?.try_into().ok()?))
}

/// 编码文档身份。
pub fn encode_doc_id(out: &mut [u8], document: DocId) -> Option<usize> {
    if out.len() < DOC_LEN {
        return None;
    }
    out[..DOC_LEN].copy_from_slice(&document.to_le_bytes());
    Some(DOC_LEN)
}

/// 解析修订身份。
pub fn decode_revision_id(bytes: &[u8]) -> Option<RevisionId> {
    Some(RevisionId::from_le_bytes(
        bytes.get(..REVISION_ID_LEN)?.try_into().ok()?,
    ))
}

/// 编码修订身份。
pub fn encode_revision_id(out: &mut [u8], revision: RevisionId) -> Option<usize> {
    if out.len() < REVISION_ID_LEN {
        return None;
    }
    out[..REVISION_ID_LEN].copy_from_slice(&revision.to_le_bytes());
    Some(REVISION_ID_LEN)
}

/// 标签视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagView<'a> {
    /// 标签编号。
    pub id: u32,
    /// 标签名字。
    pub name: &'a str,
}

/// 编码一个标签，返回写入结束位置。
pub fn encode_tag(out: &mut [u8], offset: usize, id: u32, name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() > u8::MAX as usize {
        return None;
    }
    let end = offset.checked_add(TAG_HEADER)?.checked_add(bytes.len())?;
    if end > out.len() {
        return None;
    }
    out[offset..offset + 4].copy_from_slice(&id.to_le_bytes());
    out[offset + 4] = bytes.len() as u8;
    out[offset + TAG_HEADER..end].copy_from_slice(bytes);
    Some(end)
}

/// 标签迭代器。
pub struct Tags<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Tags<'a> {
    type Item = TagView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + TAG_HEADER > self.bytes.len() {
            return None;
        }
        let id = u32::from_le_bytes(self.bytes[start..start + 4].try_into().ok()?);
        let length = self.bytes[start + 4] as usize;
        let name_start = start + TAG_HEADER;
        let end = name_start.checked_add(length)?;
        if end > self.bytes.len() {
            return None;
        }
        let name = core::str::from_utf8(&self.bytes[name_start..end]).ok()?;
        self.offset = end;
        Some(TagView { id, name })
    }
}

/// 解析标签序列。
pub fn decode_tags(bytes: &[u8]) -> Tags<'_> {
    Tags { bytes, offset: 0 }
}

/// 快照视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotView<'a> {
    /// 快照编号。
    pub id: u64,
    /// 创建时的卷代数。
    pub generation: u64,
    /// 创建时的民用时间。
    pub moment: u64,
    /// 固定的文档数量。
    pub documents: u32,
    /// 快照名字。
    pub name: &'a str,
}

/// 编码一个快照，返回写入结束位置。
pub fn encode_snapshot(
    out: &mut [u8],
    offset: usize,
    id: u64,
    generation: u64,
    moment: u64,
    documents: u32,
    name: &str,
) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() > u8::MAX as usize {
        return None;
    }
    let end = offset
        .checked_add(SNAPSHOT_HEADER)?
        .checked_add(bytes.len())?;
    if end > out.len() {
        return None;
    }
    out[offset..offset + 8].copy_from_slice(&id.to_le_bytes());
    out[offset + 8..offset + 16].copy_from_slice(&generation.to_le_bytes());
    out[offset + 16..offset + 24].copy_from_slice(&moment.to_le_bytes());
    out[offset + 24..offset + 28].copy_from_slice(&documents.to_le_bytes());
    out[offset + 28] = bytes.len() as u8;
    out[offset + SNAPSHOT_HEADER..end].copy_from_slice(bytes);
    Some(end)
}

/// 快照迭代器。
pub struct Snapshots<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Snapshots<'a> {
    type Item = SnapshotView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + SNAPSHOT_HEADER > self.bytes.len() {
            return None;
        }
        let id = u64::from_le_bytes(self.bytes[start..start + 8].try_into().ok()?);
        let generation = u64::from_le_bytes(self.bytes[start + 8..start + 16].try_into().ok()?);
        let moment = u64::from_le_bytes(self.bytes[start + 16..start + 24].try_into().ok()?);
        let documents = u32::from_le_bytes(self.bytes[start + 24..start + 28].try_into().ok()?);
        let length = self.bytes[start + 28] as usize;
        let name_start = start + SNAPSHOT_HEADER;
        let end = name_start.checked_add(length)?;
        if end > self.bytes.len() {
            return None;
        }
        let name = core::str::from_utf8(&self.bytes[name_start..end]).ok()?;
        self.offset = end;
        Some(SnapshotView {
            id,
            generation,
            moment,
            documents,
            name,
        })
    }
}

/// 解析快照序列。
pub fn decode_snapshots(bytes: &[u8]) -> Snapshots<'_> {
    Snapshots { bytes, offset: 0 }
}

/// 文档身份迭代器。
pub struct DocIds<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for DocIds<'_> {
    type Item = DocId;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        let document = decode_doc_id(self.bytes.get(start..)?)?;
        self.offset = start + DOC_LEN;
        Some(document)
    }
}

/// 解析文档身份序列。
pub fn decode_doc_ids(bytes: &[u8]) -> DocIds<'_> {
    DocIds { bytes, offset: 0 }
}

/// 解析 u32。
pub fn decode_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// 编码 u32。
pub fn encode_u32(out: &mut [u8], value: u32) -> Option<usize> {
    if out.len() < 4 {
        return None;
    }
    out[..4].copy_from_slice(&value.to_le_bytes());
    Some(4)
}

/// 解析令牌。
pub fn decode_token(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..TOKEN_LEN)?.try_into().ok()?))
}

/// 编码令牌。
pub fn encode_token(out: &mut [u8], token: u64) -> Option<usize> {
    if out.len() < TOKEN_LEN {
        return None;
    }
    out[..TOKEN_LEN].copy_from_slice(&token.to_le_bytes());
    Some(TOKEN_LEN)
}

/// 解析 u64。
pub fn decode_u64(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
}

/// 编码 u64。
pub fn encode_u64(out: &mut [u8], value: u64) -> Option<usize> {
    if out.len() < 8 {
        return None;
    }
    out[..8].copy_from_slice(&value.to_le_bytes());
    Some(8)
}

/// 文档类型编码。
pub const fn kind_code(kind: DocumentKind) -> u8 {
    match kind {
        DocumentKind::Content => KIND_CONTENT,
        DocumentKind::Collection => KIND_COLLECTION,
    }
}
