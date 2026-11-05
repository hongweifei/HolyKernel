//! 快照：固定文档表、标签视图与根修订。

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::crc32c::crc32c;
use crate::error::HolyError;
use crate::ids::{DocId, RevisionId};
use crate::store::Store;
use crate::tree::{TreeMap, load_tree, store_tree};
use crate::types::{DocumentKind, DocumentRecord, SnapshotInfo, validate_name};

use super::Volume;

const SNAPSHOT_HEADER: usize = 8 + 8 + 8 + 16 + 8 + 4 + 4 + 4 + 4 + 1;
const DOCUMENT_ENTRY: usize = 16 + 8 + 1;
const TAG_HEADER: usize = 4 + 1;
const MEMBER_ENTRY: usize = 4 + 16;
const CHECKSUM: usize = 4;
pub(super) const CHUNK: usize = 2048;

pub(crate) struct SnapshotRecord {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) generation: u64,
    pub(crate) moment: u64,
    pub(crate) root_document: DocId,
    pub(crate) root_revision: RevisionId,
    pub(crate) next_tag: u32,
    pub(crate) documents: BTreeMap<DocId, DocumentRecord>,
    pub(crate) tags: BTreeMap<u32, String>,
    pub(crate) tag_members: BTreeMap<u32, BTreeSet<DocId>>,
}

impl Clone for SnapshotRecord {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            generation: self.generation,
            moment: self.moment,
            root_document: self.root_document,
            root_revision: self.root_revision,
            next_tag: self.next_tag,
            documents: self.documents.clone(),
            tags: self.tags.clone(),
            tag_members: self.tag_members.clone(),
        }
    }
}

impl SnapshotRecord {
    pub(crate) fn info(&self) -> SnapshotInfo {
        SnapshotInfo {
            id: self.id,
            name: self.name.clone(),
            generation: self.generation,
            moment: self.moment,
            documents: self.documents.len() as u32,
        }
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, HolyError> {
        let name = self.name.as_bytes();
        let tag_count = self.tags.len();
        let member_count: usize = self.tag_members.values().map(BTreeSet::len).sum();
        let mut bytes = Vec::with_capacity(
            SNAPSHOT_HEADER
                + name.len()
                + self.documents.len() * DOCUMENT_ENTRY
                + tag_count * TAG_HEADER
                + member_count * MEMBER_ENTRY
                + CHECKSUM,
        );
        bytes.extend_from_slice(&self.id.to_le_bytes());
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        bytes.extend_from_slice(&self.moment.to_le_bytes());
        bytes.extend_from_slice(&self.root_document.to_le_bytes());
        bytes.extend_from_slice(&self.root_revision.to_le_bytes());
        bytes.extend_from_slice(&self.next_tag.to_le_bytes());
        bytes.extend_from_slice(&(self.documents.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(tag_count as u32).to_le_bytes());
        bytes.extend_from_slice(&(member_count as u32).to_le_bytes());
        bytes.push(name.len() as u8);
        bytes.extend_from_slice(name);
        for (document, record) in &self.documents {
            bytes.extend_from_slice(&document.to_le_bytes());
            bytes.extend_from_slice(&record.head.to_le_bytes());
            bytes.push(match record.kind {
                DocumentKind::Content => 0,
                DocumentKind::Collection => 1,
            });
        }
        for (tag, name) in &self.tags {
            let name = name.as_bytes();
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.push(name.len() as u8);
            bytes.extend_from_slice(name);
        }
        for (tag, members) in &self.tag_members {
            for document in members {
                bytes.extend_from_slice(&tag.to_le_bytes());
                bytes.extend_from_slice(&document.to_le_bytes());
            }
        }
        let checksum = crc32c(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        Ok(bytes)
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, HolyError> {
        if bytes.len() < SNAPSHOT_HEADER + CHECKSUM {
            return Err(HolyError::Corrupt);
        }
        let (payload, checksum) = bytes.split_at(bytes.len() - CHECKSUM);
        let stored = u32::from_le_bytes(checksum.try_into().map_err(|_| HolyError::Corrupt)?);
        if crc32c(payload) != stored {
            return Err(HolyError::Corrupt);
        }
        let mut reader = Reader::new(payload);
        let id = reader.u64()?;
        let generation = reader.u64()?;
        let moment = reader.u64()?;
        let root_document = DocId::from_le_bytes(
            reader
                .take(16)?
                .try_into()
                .map_err(|_| HolyError::Corrupt)?,
        );
        let root_revision =
            RevisionId::from_le_bytes(reader.take(8)?.try_into().map_err(|_| HolyError::Corrupt)?);
        let next_tag = reader.u32()?;
        let document_count = reader.u32()? as usize;
        let tag_count = reader.u32()? as usize;
        let member_count = reader.u32()? as usize;
        let name_len = reader.u8()? as usize;
        let name = core::str::from_utf8(reader.take(name_len)?).map_err(|_| HolyError::Corrupt)?;
        let mut documents = BTreeMap::new();
        for _ in 0..document_count {
            let document = DocId::from_le_bytes(
                reader
                    .take(16)?
                    .try_into()
                    .map_err(|_| HolyError::Corrupt)?,
            );
            let head = RevisionId::from_le_bytes(
                reader.take(8)?.try_into().map_err(|_| HolyError::Corrupt)?,
            );
            let kind = match reader.u8()? {
                0 => DocumentKind::Content,
                1 => DocumentKind::Collection,
                _ => return Err(HolyError::Corrupt),
            };
            if documents
                .insert(
                    document,
                    DocumentRecord {
                        document,
                        head,
                        kind,
                    },
                )
                .is_some()
            {
                return Err(HolyError::Corrupt);
            }
        }
        let mut tags = BTreeMap::new();
        for _ in 0..tag_count {
            let tag = reader.u32()?;
            let len = reader.u8()? as usize;
            let name = core::str::from_utf8(reader.take(len)?).map_err(|_| HolyError::Corrupt)?;
            if tags.insert(tag, String::from(name)).is_some() {
                return Err(HolyError::Corrupt);
            }
        }
        let mut tag_members: BTreeMap<u32, BTreeSet<DocId>> = BTreeMap::new();
        for _ in 0..member_count {
            let tag = reader.u32()?;
            let document = DocId::from_le_bytes(
                reader
                    .take(16)?
                    .try_into()
                    .map_err(|_| HolyError::Corrupt)?,
            );
            if !tags.contains_key(&tag) || !documents.contains_key(&document) {
                return Err(HolyError::Corrupt);
            }
            tag_members.entry(tag).or_default().insert(document);
        }
        if reader.remaining() != 0 {
            return Err(HolyError::Corrupt);
        }
        let record = Self {
            id,
            name: String::from(name),
            generation,
            moment,
            root_document,
            root_revision,
            next_tag: next_tag.max(1),
            documents,
            tags,
            tag_members,
        };
        let root = record
            .documents
            .get(&record.root_document)
            .ok_or(HolyError::Corrupt)?;
        if root.head != record.root_revision {
            return Err(HolyError::Corrupt);
        }
        Ok(record)
    }
}

pub(super) fn store<D: BlockDevice>(
    store: &mut Store<D>,
    snapshots: &BTreeMap<u64, SnapshotRecord>,
) -> Result<u64, HolyError> {
    let mut tree = TreeMap::new();
    for (id, snapshot) in snapshots {
        let bytes = snapshot.encode()?;
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            let mut key = Vec::with_capacity(12);
            key.extend_from_slice(&id.to_le_bytes());
            key.extend_from_slice(&(index as u32).to_le_bytes());
            tree.insert(key, chunk.to_vec());
        }
    }
    store_tree(store, &tree)
}

pub(super) fn load<D: BlockDevice>(
    store: &Store<D>,
    root: u64,
) -> Result<BTreeMap<u64, SnapshotRecord>, HolyError> {
    let mut snapshots = BTreeMap::new();
    if root == 0 {
        return Ok(snapshots);
    }
    let tree = load_tree(store, root)?;
    let mut chunks: BTreeMap<u64, BTreeMap<u32, Vec<u8>>> = BTreeMap::new();
    for (key, value) in tree {
        if key.len() != 12 {
            return Err(HolyError::Corrupt);
        }
        let id = u64::from_le_bytes(key[..8].try_into().map_err(|_| HolyError::Corrupt)?);
        let index = u32::from_le_bytes(key[8..12].try_into().map_err(|_| HolyError::Corrupt)?);
        if chunks.entry(id).or_default().insert(index, value).is_some() {
            return Err(HolyError::Corrupt);
        }
    }
    for (id, parts) in chunks {
        let count = parts.len() as u32;
        let mut bytes = Vec::new();
        for index in 0..count {
            let part = parts.get(&index).ok_or(HolyError::Corrupt)?;
            bytes.extend_from_slice(part);
        }
        let record = SnapshotRecord::decode(&bytes)?;
        if record.id != id {
            return Err(HolyError::Corrupt);
        }
        snapshots.insert(id, record);
    }
    Ok(snapshots)
}

impl<D: BlockDevice> Volume<D> {
    /// 固定当前文档表、标签视图与根修订。
    pub(crate) fn snapshot(&mut self, name: &str) -> Result<u64, HolyError> {
        validate_name(name)?;
        let root = self.documents.get(&DocId::ROOT).ok_or(HolyError::Corrupt)?;
        let id = self.next_snapshot;
        self.next_snapshot = self.next_snapshot.saturating_add(1);
        let record = SnapshotRecord {
            id,
            name: String::from(name),
            generation: self.superblock.generation,
            moment: 0,
            root_document: DocId::ROOT,
            root_revision: root.head,
            next_tag: self.next_tag,
            documents: self.documents.clone(),
            tags: self.tags.clone(),
            tag_members: self.tag_members.clone(),
        };
        self.snapshots.insert(id, record);
        self.dirty = true;
        if let Err(error) = self.persist() {
            self.snapshots.remove(&id);
            return Err(error);
        }
        Ok(id)
    }

    /// 枚举快照。
    pub(crate) fn snapshots(&self) -> Vec<SnapshotInfo> {
        self.snapshots.values().map(SnapshotRecord::info).collect()
    }

    /// 把卷切回快照的文档表、标签视图与根修订。
    pub(crate) fn restore(&mut self, id: u64) -> Result<u64, HolyError> {
        let snapshot = self
            .snapshots
            .get(&id)
            .cloned()
            .ok_or(HolyError::NotFound)?;
        for record in snapshot.documents.values() {
            if !self.revisions.contains_key(&record.head) {
                return Err(HolyError::Corrupt);
            }
        }
        let documents = core::mem::replace(&mut self.documents, snapshot.documents);
        let tags = core::mem::replace(&mut self.tags, snapshot.tags);
        let tag_members = core::mem::replace(&mut self.tag_members, snapshot.tag_members);
        let next_tag = core::mem::replace(&mut self.next_tag, snapshot.next_tag);
        self.dirty = true;
        if let Err(error) = self.persist() {
            self.documents = documents;
            self.tags = tags;
            self.tag_members = tag_members;
            self.next_tag = next_tag;
            return Err(error);
        }
        Ok(self.superblock.generation)
    }

    /// 释放快照固定。
    pub(crate) fn forget(&mut self, id: u64) -> Result<(), HolyError> {
        let record = self.snapshots.remove(&id).ok_or(HolyError::NotFound)?;
        self.dirty = true;
        if let Err(error) = self.persist() {
            self.snapshots.insert(id, record);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn generation(&self) -> u64 {
        self.superblock.generation
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], HolyError> {
        let end = self.offset.checked_add(length).ok_or(HolyError::Corrupt)?;
        let bytes = self.bytes.get(self.offset..end).ok_or(HolyError::Corrupt)?;
        self.offset = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, HolyError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, HolyError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| HolyError::Corrupt)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, HolyError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| HolyError::Corrupt)?,
        ))
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}
