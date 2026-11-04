//! 卷状态、文档/修订/集合操作与提交。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use hk_block::BlockDevice;
use hk_vfs::{DirEntry, FileType, Metadata, NodeId};

use crate::error::{HolyError, map_io};
use crate::ids::{DocId, RevisionId};
use crate::store::Store;
use crate::superblock::{BLOCK_SIZE, SUPERBLOCK_A, SUPERBLOCK_B, SUPERBLOCK_SIZE, Superblock};
use crate::tree::{TreeMap, collect_blocks, load_tree, store_tree};
use crate::types::{
    DocumentKind, DocumentRecord, EntryRecord, Extent, MAX_EXTENTS, PayloadKind, RevisionFlags,
    RevisionRecord, validate_name,
};

type Entries = BTreeMap<Vec<u8>, EntryRecord>;

const MAX_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct Volume<D: BlockDevice> {
    store: Store<D>,
    superblock: Superblock,
    active_slot: u64,
    documents: BTreeMap<DocId, DocumentRecord>,
    revisions: BTreeMap<RevisionId, RevisionRecord>,
    collections: BTreeMap<RevisionId, Entries>,
    nodes: BTreeMap<DocId, NodeId>,
    documents_by_node: BTreeMap<NodeId, DocId>,
    next_node: NodeId,
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
            nodes: BTreeMap::new(),
            documents_by_node: BTreeMap::new(),
            next_node: 2,
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
        volume.nodes.insert(DocId::ROOT, 1);
        volume.documents_by_node.insert(1, DocId::ROOT);
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
        let mut nodes = BTreeMap::new();
        let mut documents_by_node = BTreeMap::new();
        nodes.insert(DocId::ROOT, 1);
        documents_by_node.insert(1, DocId::ROOT);
        Ok(Self {
            store,
            superblock,
            active_slot,
            documents,
            revisions,
            collections,
            nodes,
            documents_by_node,
            next_node: 2,
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

    pub(crate) fn root_node(&self) -> NodeId {
        1
    }

    pub(crate) fn lookup(&mut self, parent: NodeId, name: &str) -> Result<NodeId, HolyError> {
        let parent_doc = self.document_for_node(parent)?;
        let head = self.head(parent_doc)?;
        if !head.payload_has_collection() {
            return Err(HolyError::NotDirectory);
        }
        let entries = self.collections.get(&head.id).ok_or(HolyError::Corrupt)?;
        let entry = entries.get(name.as_bytes()).ok_or(HolyError::NotFound)?;
        self.node_for(entry.document)
    }

    pub(crate) fn metadata(&self, node: NodeId) -> Result<Metadata, HolyError> {
        let document = self.document_for_node(node)?;
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        Ok(Metadata {
            kind: record.kind.file_type(),
            size: if record.kind == DocumentKind::Content {
                revision.size
            } else {
                0
            },
        })
    }

    pub(crate) fn read(
        &self,
        node: NodeId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, HolyError> {
        let document = self.document_for_node(node)?;
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

    pub(crate) fn write(
        &mut self,
        node: NodeId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, HolyError> {
        let document = self.document_for_node(node)?;
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

    pub(crate) fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), HolyError> {
        if size > MAX_CONTENT_BYTES {
            return Err(HolyError::NoSpace);
        }
        let document = self.document_for_node(node)?;
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
        parent: NodeId,
        name: &str,
        kind: FileType,
    ) -> Result<NodeId, HolyError> {
        validate_name(name)?;
        let parent_doc = self.document_for_node(parent)?;
        let parent_head = *self.head(parent_doc)?;
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
        let document_kind = DocumentKind::from_file_type(kind);
        let revision_kind = match kind {
            FileType::File => PayloadKind::Content,
            FileType::Directory => PayloadKind::Collection,
        };
        let mut record = RevisionRecord::empty(revision, document, None);
        record.payload = revision_kind;
        self.revisions.insert(revision, record);
        self.documents.insert(
            document,
            DocumentRecord {
                document,
                head: revision,
                kind: document_kind,
            },
        );
        if kind == FileType::Directory {
            self.collections.insert(revision, BTreeMap::new());
        }
        let mut next_entries = entries;
        next_entries.insert(
            name.as_bytes().to_vec(),
            EntryRecord {
                document,
                kind: document_kind,
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
        if let Some(record) = self.documents.get_mut(&parent_doc) {
            record.head = parent_revision;
        }
        self.dirty = true;
        self.node_for(document)
    }

    pub(crate) fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, HolyError> {
        let document = self.document_for_node(node)?;
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind != DocumentKind::Collection {
            return Err(HolyError::NotDirectory);
        }
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        let entries = self
            .collections
            .get(&revision.id)
            .ok_or(HolyError::Corrupt)?;
        let mut result = Vec::with_capacity(entries.len());
        for (name, entry) in entries {
            let name = core::str::from_utf8(name).map_err(|_| HolyError::Corrupt)?;
            result.push(DirEntry {
                name: String::from(name),
                kind: entry.kind.file_type(),
            });
        }
        Ok(result)
    }

    pub(crate) fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, HolyError> {
        let parent_doc = self.document_for_node(parent)?;
        let parent_head = *self.head(parent_doc)?;
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
        if let Some(record) = self.documents.get_mut(&parent_doc) {
            record.head = parent_revision;
        }
        self.dirty = true;
        self.node_for(entry.document)
    }

    fn head(&self, document: DocId) -> Result<&RevisionRecord, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        self.revisions.get(&record.head).ok_or(HolyError::Corrupt)
    }

    fn document_for_node(&self, node: NodeId) -> Result<DocId, HolyError> {
        self.documents_by_node
            .get(&node)
            .copied()
            .ok_or(HolyError::NotFound)
    }

    fn node_for(&mut self, document: DocId) -> Result<NodeId, HolyError> {
        if let Some(node) = self.nodes.get(&document) {
            return Ok(*node);
        }
        if !self.documents.contains_key(&document) {
            return Err(HolyError::NotFound);
        }
        let node = self.next_node;
        self.next_node = self.next_node.saturating_add(1);
        self.nodes.insert(document, node);
        self.documents_by_node.insert(node, document);
        Ok(node)
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
