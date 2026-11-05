//! 卷状态、挂载与提交。
//!
//! 文档内容、集合/元数据、标签与持久化分别位于子模块。

mod content;
mod metadata;
mod persist;
mod tags;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::{HolyError, map_io};
use crate::ids::{DocId, RevisionId};
use crate::store::Store;
use crate::superblock::{BLOCK_SIZE, SUPERBLOCK_A, SUPERBLOCK_B, Superblock};
use crate::tree::load_tree;
use crate::types::{
    DocumentKind, DocumentRecord, EntryRecord, Extent, MAX_EXTENTS, PayloadKind, RevisionFlags,
    RevisionRecord,
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
    tags: BTreeMap<u32, String>,
    tag_members: BTreeMap<u32, BTreeSet<DocId>>,
    next_tag: u32,
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
            tags: BTreeMap::new(),
            tag_members: BTreeMap::new(),
            next_tag: 1,
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
        let mut tags = BTreeMap::new();
        let mut tag_members: BTreeMap<u32, BTreeSet<DocId>> = BTreeMap::new();
        if superblock.tag_tree != 0 {
            let tree = load_tree(&store, superblock.tag_tree)?;
            for (key, value) in tree {
                if key.len() == 5 && key[0] == 0 {
                    let id =
                        u32::from_le_bytes(key[1..5].try_into().map_err(|_| HolyError::Corrupt)?);
                    let name = core::str::from_utf8(&value).map_err(|_| HolyError::Corrupt)?;
                    tags.insert(id, String::from(name));
                } else if key.len() == 21 && key[0] == 1 {
                    let id =
                        u32::from_le_bytes(key[1..5].try_into().map_err(|_| HolyError::Corrupt)?);
                    let document = DocId::from_le_bytes(
                        key[5..21].try_into().map_err(|_| HolyError::Corrupt)?,
                    );
                    tag_members.entry(id).or_default().insert(document);
                } else {
                    return Err(HolyError::Corrupt);
                }
            }
        }
        for (tag, members) in &tag_members {
            if !tags.contains_key(tag) {
                return Err(HolyError::Corrupt);
            }
            for document in members {
                if !documents.contains_key(document) {
                    return Err(HolyError::Corrupt);
                }
            }
        }
        Ok(Self {
            store,
            superblock,
            active_slot,
            documents,
            revisions,
            collections,
            tags,
            tag_members,
            next_tag: superblock.next_tag,
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
