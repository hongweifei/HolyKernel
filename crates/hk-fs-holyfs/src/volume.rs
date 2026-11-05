//! 卷状态、文档/修订/集合操作与提交。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::{HolyError, map_io};
use crate::ids::{DocId, RevisionId};
use crate::store::Store;
use crate::superblock::{BLOCK_SIZE, SUPERBLOCK_A, SUPERBLOCK_B, SUPERBLOCK_SIZE, Superblock};
use crate::tree::{TreeMap, collect_blocks, load_tree, store_tree};
use crate::types::{
    DocumentInfo, DocumentKind, DocumentRecord, EntryInfo, EntryRecord, Extent, MAX_EXTENTS,
    PayloadKind, RevisionFlags, RevisionInfo, RevisionRecord, validate_name,
};

type Entries = BTreeMap<Vec<u8>, EntryRecord>;

const MAX_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

/// 修订链枚举上限。
const MAX_REVISIONS: usize = 4096;

pub(crate) struct Volume<D: BlockDevice> {
    store: Store<D>,
    superblock: Superblock,
    active_slot: u64,
    documents: BTreeMap<DocId, DocumentRecord>,
    revisions: BTreeMap<RevisionId, RevisionRecord>,
    collections: BTreeMap<RevisionId, Entries>,
    dirty: bool,
}

impl<D: BlockDevice> Volume<D> {
    pub(crate) fn format(device: D) -> Result<D, HolyError> {
        let store = Store::format(device)?;
        let bitmap_blocks = store.bitmap_blocks()?;
        let superblock = Superblock::empty(store.total_blocks(), bitmap_blocks);
        let root = RevisionRecord {
            id: RevisionId::ROOT,
            document: DocId::ROOT,
            parent: None,
            payload: PayloadKind::Collection,
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
        };
        let mut volume = Self {
            store,
            superblock,
            active_slot: SUPERBLOCK_B,
            documents: BTreeMap::new(),
            revisions: BTreeMap::new(),
            collections: BTreeMap::new(),
            dirty: true,
        };
        volume.documents.insert(
            DocId::ROOT,
            DocumentRecord {
                document: DocId::ROOT,
                head: RevisionId::ROOT,
                kind: DocumentKind::Collection,
            },
        );
        volume.revisions.insert(RevisionId::ROOT, root);
        volume.collections.insert(RevisionId::ROOT, BTreeMap::new());
        volume.persist()?;
        Ok(volume.store.into_device())
    }

    pub(crate) fn mount(device: D) -> Result<Self, HolyError> {
        let superblock = read_superblock(&device)?;
        let active_slot = if superblock.0 == SUPERBLOCK_A {
            SUPERBLOCK_A
        } else {
            SUPERBLOCK_B
        };
        let superblock = superblock.1;
        let store = Store::load(device, &superblock)?;
        let document_map = load_tree(&store, superblock.document_tree)?;
        let revision_map = load_tree(&store, superblock.revision_tree)?;
        let mut documents = BTreeMap::new();
        for value in document_map.values() {
            let record = DocumentRecord::decode(value)?;
            documents.insert(record.document, record);
        }
        let mut revisions = BTreeMap::new();
        for value in revision_map.values() {
            let record = RevisionRecord::decode(value)?;
            revisions.insert(record.id, record);
        }
        let mut collections = BTreeMap::new();
        for revision in revisions.values() {
            if revision.entries_root == 0 {
                if revision.payload != PayloadKind::Content {
                    collections.insert(revision.id, BTreeMap::new());
                }
                continue;
            }
            let tree = load_tree(&store, revision.entries_root)?;
            let mut entries = BTreeMap::new();
            for (name, value) in tree {
                entries.insert(name, EntryRecord::decode(&value)?);
            }
            collections.insert(revision.id, entries);
        }
        let root = documents.get(&DocId::ROOT).ok_or(HolyError::Corrupt)?;
        if !revisions.contains_key(&root.head) {
            return Err(HolyError::Corrupt);
        }
        Ok(Self {
            store,
            superblock,
            active_slot,
            documents,
            revisions,
            collections,
            dirty: false,
        })
    }

    pub(crate) fn sync(&mut self) -> Result<(), HolyError> {
        if self.dirty {
            self.persist()
        } else {
            self.store.flush()
        }
    }

    pub(crate) fn root_document(&self) -> DocId {
        DocId::ROOT
    }

    pub(crate) fn lookup(&self, parent: DocId, name: &str) -> Result<DocId, HolyError> {
        let head = self.head(parent)?;
        if !head.payload_has_collection() {
            return Err(HolyError::NotDirectory);
        }
        let entries = self.collections.get(&head.id).ok_or(HolyError::Corrupt)?;
        let entry = entries.get(name.as_bytes()).ok_or(HolyError::NotFound)?;
        Ok(entry.document)
    }

    pub(crate) fn read_range(
        &self,
        document: DocId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        if offset >= revision.size || buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min((revision.size - offset) as usize);
        self.read_extents(revision, offset, &mut buffer[..length])?;
        Ok(length)
    }

    pub(crate) fn write_range(
        &mut self,
        document: DocId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let head = *self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(HolyError::Invalid)?;
        if end > MAX_CONTENT_BYTES {
            return Err(HolyError::NoSpace);
        }
        if offset == head.size && head.size.is_multiple_of(BLOCK_SIZE as u64) {
            self.append_content(document, &head, offset, data)?;
        } else {
            self.rewrite_content(document, &head, offset, data)?;
        }
        Ok(data.len())
    }

    pub(crate) fn truncate(&mut self, document: DocId, size: u64) -> Result<(), HolyError> {
        if size > MAX_CONTENT_BYTES {
            return Err(HolyError::NoSpace);
        }
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let head = *self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        let mut content = vec![0_u8; size as usize];
        let copy = head.size.min(size) as usize;
        if copy > 0 {
            self.read_extents(&head, 0, &mut content[..copy])?;
        }
        self.replace_content(document, &head, &content)
    }

    pub(crate) fn create(
        &mut self,
        parent: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, HolyError> {
        validate_name(name)?;
        let parent_head = *self.head(parent)?;
        if !parent_head.payload_has_collection() {
            return Err(HolyError::NotDirectory);
        }
        let entries = self
            .collections
            .get(&parent_head.id)
            .ok_or(HolyError::Corrupt)?
            .clone();
        if entries.contains_key(name.as_bytes()) {
            return Err(HolyError::Exists);
        }
        let document = self.allocate_document();
        let revision = self.allocate_revision();
        let revision_kind = match kind {
            DocumentKind::Content => PayloadKind::Content,
            DocumentKind::Collection => PayloadKind::Collection,
        };
        let mut record = RevisionRecord::empty(revision, document, None);
        record.payload = revision_kind;
        self.revisions.insert(revision, record);
        self.documents.insert(
            document,
            DocumentRecord {
                document,
                head: revision,
                kind,
            },
        );
        if kind == DocumentKind::Collection {
            self.collections.insert(revision, BTreeMap::new());
        }
        let mut next_entries = entries;
        next_entries.insert(
            name.as_bytes().to_vec(),
            EntryRecord {
                document,
                kind,
                flags: 0,
            },
        );
        let parent_revision = self.allocate_revision();
        let mut parent_record = parent_head;
        parent_record.id = parent_revision;
        parent_record.parent = Some(parent_head.id);
        parent_record.entries_root = 0;
        self.revisions.insert(parent_revision, parent_record);
        self.collections.insert(parent_revision, next_entries);
        if let Some(record) = self.documents.get_mut(&parent) {
            record.head = parent_revision;
        }
        self.dirty = true;
        Ok(document)
    }

    pub(crate) fn unlink(&mut self, parent: DocId, name: &str) -> Result<DocId, HolyError> {
        let parent_head = *self.head(parent)?;
        if !parent_head.payload_has_collection() {
            return Err(HolyError::NotDirectory);
        }
        let entries = self
            .collections
            .get(&parent_head.id)
            .ok_or(HolyError::Corrupt)?
            .clone();
        let entry = *entries.get(name.as_bytes()).ok_or(HolyError::NotFound)?;
        if entry.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let mut next_entries = entries;
        next_entries.remove(name.as_bytes());
        let parent_revision = self.allocate_revision();
        let mut parent_record = parent_head;
        parent_record.id = parent_revision;
        parent_record.parent = Some(parent_head.id);
        parent_record.entries_root = 0;
        self.revisions.insert(parent_revision, parent_record);
        self.collections.insert(parent_revision, next_entries);
        if let Some(record) = self.documents.get_mut(&parent) {
            record.head = parent_revision;
        }
        self.dirty = true;
        Ok(entry.document)
    }

    /// 按 DocPath 解析文档；驱动号 `H:` 与 `.` 被忽略，`..` 不属于 Holy 命名。
    pub(crate) fn resolve(&self, path: &str) -> Result<DocId, HolyError> {
        let mut document = DocId::ROOT;
        for (index, component) in path.split('/').enumerate() {
            if component.is_empty() || component == "." {
                continue;
            }
            if index == 0 && component.ends_with(':') {
                continue;
            }
            if component == ".." {
                return Err(HolyError::Invalid);
            }
            let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
            let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
            if !revision.payload_has_collection() {
                return Err(HolyError::NotDirectory);
            }
            let entries = self
                .collections
                .get(&revision.id)
                .ok_or(HolyError::Corrupt)?;
            let entry = entries
                .get(component.as_bytes())
                .ok_or(HolyError::NotFound)?;
            document = entry.document;
        }
        Ok(document)
    }

    /// 枚举集合文档的名字、身份与类型。
    pub(crate) fn enumerate(&self, document: DocId) -> Result<Vec<EntryInfo>, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        let entries = self
            .collections
            .get(&revision.id)
            .ok_or(HolyError::NotDirectory)?;
        let mut result = Vec::with_capacity(entries.len());
        for (name, entry) in entries {
            let name = core::str::from_utf8(name).map_err(|_| HolyError::Corrupt)?;
            result.push(EntryInfo {
                name: String::from(name),
                document: entry.document,
                kind: entry.kind,
            });
        }
        Ok(result)
    }

    /// 返回文档身份、类型与当前修订。
    pub(crate) fn document_info(&self, document: DocId) -> Result<DocumentInfo, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        Ok(DocumentInfo {
            id: document,
            kind: record.kind,
            head: revision.id,
            parent: revision.parent,
            size: revision.size,
            moment: revision.moment,
        })
    }

    /// 整文档读取。
    pub(crate) fn read_document(&self, document: DocId) -> Result<Vec<u8>, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        let size = revision.size as usize;
        let mut buffer = vec![0_u8; size];
        if size > 0 {
            self.read_extents(revision, 0, &mut buffer)?;
        }
        Ok(buffer)
    }

    /// 从 head 出发枚举修订链。
    pub(crate) fn revisions(&self, document: DocId) -> Result<Vec<RevisionInfo>, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        let mut result = Vec::new();
        let mut current = Some(record.head);
        while let Some(id) = current {
            if result.len() > MAX_REVISIONS {
                return Err(HolyError::Corrupt);
            }
            let revision = self.revisions.get(&id).ok_or(HolyError::Corrupt)?;
            result.push(RevisionInfo {
                id: revision.id,
                parent: revision.parent,
                size: revision.size,
                moment: revision.moment,
                flags: revision.flags,
            });
            current = revision.parent;
        }
        Ok(result)
    }

    /// 整文档写入，生成新的内容修订并返回其身份。
    pub(crate) fn write_document(
        &mut self,
        document: DocId,
        content: &[u8],
    ) -> Result<RevisionId, HolyError> {
        if content.len() as u64 > MAX_CONTENT_BYTES {
            return Err(HolyError::NoSpace);
        }
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let head = *self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        self.replace_content(document, &head, content)?;
        let record = self.documents.get(&document).ok_or(HolyError::Corrupt)?;
        Ok(record.head)
    }

    fn head(&self, document: DocId) -> Result<&RevisionRecord, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        self.revisions.get(&record.head).ok_or(HolyError::Corrupt)
    }

    fn allocate_document(&mut self) -> DocId {
        let document = self.superblock.next_document;
        self.superblock.next_document = DocId::new(document.get().saturating_add(1));
        document
    }

    fn allocate_revision(&mut self) -> RevisionId {
        let revision = RevisionId::new(self.superblock.next_revision);
        self.superblock.next_revision = self.superblock.next_revision.saturating_add(1);
        revision
    }

    fn read_extents(
        &self,
        revision: &RevisionRecord,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<(), HolyError> {
        let mut position = 0_u64;
        let mut done = 0;
        for extent in revision.extents() {
            let bytes = extent.bytes();
            if done == buffer.len() {
                break;
            }
            if offset + (done as u64) < position + bytes {
                let within = (offset + done as u64).saturating_sub(position) as usize;
                let take = (bytes as usize - within).min(buffer.len() - done);
                self.store.read_bytes(
                    extent.start * BLOCK_SIZE as u64 + within as u64,
                    &mut buffer[done..done + take],
                )?;
                done += take;
            }
            position += bytes;
        }
        if done != buffer.len() {
            return Err(HolyError::Corrupt);
        }
        Ok(())
    }

    fn append_content(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        offset: u64,
        data: &[u8],
    ) -> Result<(), HolyError> {
        let blocks = data.len().div_ceil(BLOCK_SIZE) as u64;
        let start = self.store.allocate_blocks(blocks)?;
        self.store.write_bytes(start * BLOCK_SIZE as u64, data)?;
        let mut extents = Vec::with_capacity(head.extent_count as usize + 1);
        extents.extend_from_slice(head.extents());
        if let Some(last) = extents.last_mut()
            && last.end() == start
            && last.blocks.checked_add(blocks as u32).is_some()
        {
            last.blocks += blocks as u32;
        } else {
            extents.push(Extent {
                start,
                blocks: blocks as u32,
            });
        }
        if extents.len() > MAX_EXTENTS {
            self.store.free_blocks(start, blocks);
            return self.rewrite_content(document, head, offset, data);
        }
        self.insert_content_revision(document, head, extents, offset + data.len() as u64)
    }

    fn rewrite_content(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        offset: u64,
        data: &[u8],
    ) -> Result<(), HolyError> {
        let new_size = head.size.max(offset + data.len() as u64);
        if new_size > MAX_CONTENT_BYTES {
            return Err(HolyError::NoSpace);
        }
        let mut content = vec![0_u8; new_size as usize];
        if head.size > 0 {
            self.read_extents(head, 0, &mut content[..head.size as usize])?;
        }
        content[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        self.replace_content(document, head, &content)
    }

    fn replace_content(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        content: &[u8],
    ) -> Result<(), HolyError> {
        let blocks = content.len().div_ceil(BLOCK_SIZE) as u64;
        let extents = if blocks == 0 {
            Vec::new()
        } else {
            let start = self.store.allocate_blocks(blocks)?;
            self.store.write_bytes(start * BLOCK_SIZE as u64, content)?;
            vec![Extent {
                start,
                blocks: blocks as u32,
            }]
        };
        self.insert_content_revision(document, head, extents, content.len() as u64)
    }

    fn insert_content_revision(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        extents: Vec<Extent>,
        size: u64,
    ) -> Result<(), HolyError> {
        let revision = self.allocate_revision();
        let mut record = RevisionRecord::empty(revision, document, Some(head.id));
        record.extent_count = extents.len() as u16;
        for (slot, extent) in record.extents.iter_mut().zip(extents) {
            *slot = extent;
        }
        record.size = size;
        self.revisions.insert(revision, record);
        if let Some(document) = self.documents.get_mut(&document) {
            document.head = revision;
        }
        self.dirty = true;
        Ok(())
    }

    fn persist(&mut self) -> Result<(), HolyError> {
        let mut old_roots = vec![self.superblock.document_tree, self.superblock.revision_tree];
        old_roots.extend(
            self.revisions
                .values()
                .map(|revision| revision.entries_root)
                .filter(|root| *root != 0),
        );
        let collection_ids: Vec<_> = self.collections.keys().copied().collect();
        for revision_id in collection_ids {
            let entries = self
                .collections
                .get(&revision_id)
                .cloned()
                .unwrap_or_default();
            let mut tree = TreeMap::new();
            for (name, entry) in entries {
                tree.insert(name, entry.encode().to_vec());
            }
            let root = store_tree(&mut self.store, &tree)?;
            if let Some(revision) = self.revisions.get_mut(&revision_id) {
                revision.entries_root = root;
            }
        }
        for revision in self.revisions.values_mut() {
            if !revision.payload_has_collection() {
                revision.entries_root = 0;
            }
        }
        let mut documents = TreeMap::new();
        for record in self.documents.values() {
            documents.insert(
                record.document.to_le_bytes().to_vec(),
                record.encode().to_vec(),
            );
        }
        let mut revisions = TreeMap::new();
        for record in self.revisions.values() {
            revisions.insert(record.id.to_le_bytes().to_vec(), record.encode().to_vec());
        }
        let document_tree = store_tree(&mut self.store, &documents)?;
        let revision_tree = store_tree(&mut self.store, &revisions)?;
        self.superblock.generation = self.superblock.generation.saturating_add(1);
        self.superblock.total_blocks = self.store.total_blocks();
        self.superblock.bitmap_start = crate::superblock::BITMAP_START;
        self.superblock.bitmap_blocks = self.store.bitmap_blocks()?;
        self.superblock.document_tree = document_tree;
        self.superblock.revision_tree = revision_tree;
        self.store.write_bitmap()?;
        let slot = if self.active_slot == SUPERBLOCK_A {
            SUPERBLOCK_B
        } else {
            SUPERBLOCK_A
        };
        let mut block = [0_u8; BLOCK_SIZE];
        block[..SUPERBLOCK_SIZE].copy_from_slice(&self.superblock.encode());
        self.store.write_block(slot, &block)?;
        self.active_slot = slot;
        self.store.flush()?;
        self.dirty = false;

        let mut blocks = Vec::new();
        for root in old_roots {
            let _ = collect_blocks(&self.store, root, &mut blocks);
        }
        if !blocks.is_empty() {
            for block in blocks {
                self.store.free_blocks(block, 1);
            }
            self.store.write_bitmap()?;
            self.store.flush()?;
        }
        Ok(())
    }
}

fn read_superblock<D: BlockDevice>(device: &D) -> Result<(u64, Superblock), HolyError> {
    let block_size = device.block_size();
    if block_size == 0 || !BLOCK_SIZE.is_multiple_of(block_size) {
        return Err(HolyError::Invalid);
    }
    let sectors = (BLOCK_SIZE / block_size) as u64;
    let mut a = [0_u8; BLOCK_SIZE];
    let mut b = [0_u8; BLOCK_SIZE];
    device
        .read_blocks(SUPERBLOCK_A * sectors, &mut a)
        .map_err(map_io)?;
    device
        .read_blocks(SUPERBLOCK_B * sectors, &mut b)
        .map_err(map_io)?;
    let a = Superblock::decode(&a).ok();
    let b = Superblock::decode(&b).ok();
    match (a, b) {
        (Some(a), Some(b)) => {
            if b.generation > a.generation {
                Ok((SUPERBLOCK_B, b))
            } else {
                Ok((SUPERBLOCK_A, a))
            }
        }
        (Some(a), None) => Ok((SUPERBLOCK_A, a)),
        (None, Some(b)) => Ok((SUPERBLOCK_B, b)),
        (None, None) => Err(HolyError::Corrupt),
    }
}
