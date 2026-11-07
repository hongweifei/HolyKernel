//! 元数据树与超级块提交。

use alloc::vec;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::superblock::{BLOCK_SIZE, SUPERBLOCK_A, SUPERBLOCK_B, SUPERBLOCK_SIZE};
use crate::tree::{TreeMap, collect_blocks, store_tree};

use super::Volume;

impl<D: BlockDevice> Volume<D> {
    pub(super) fn persist(&mut self) -> Result<(), HolyError> {
        self.ensure_writable()?;
        let result = self.persist_inner();
        if let Err(error) = &result {
            self.note_error(error);
        }
        result
    }

    fn persist_inner(&mut self) -> Result<(), HolyError> {
        let mut old_roots = vec![
            self.superblock.document_tree,
            self.superblock.revision_tree,
            self.superblock.tag_tree,
            self.superblock.snapshot_tree,
        ];
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
        let mut tags = TreeMap::new();
        for (tag, name) in &self.tags {
            let mut key = vec![0_u8];
            key.extend_from_slice(&tag.to_le_bytes());
            tags.insert(key, name.as_bytes().to_vec());
        }
        for (tag, members) in &self.tag_members {
            for document in members {
                let mut key = vec![1_u8];
                key.extend_from_slice(&tag.to_le_bytes());
                key.extend_from_slice(&document.to_le_bytes());
                tags.insert(key, Vec::new());
            }
        }
        let document_tree = store_tree(&mut self.store, &documents)?;
        let revision_tree = store_tree(&mut self.store, &revisions)?;
        let tag_tree = store_tree(&mut self.store, &tags)?;
        let snapshot_tree = super::snapshot::store(&mut self.store, &self.snapshots)?;
        self.superblock.generation = self.superblock.generation.saturating_add(1);
        self.superblock.total_blocks = self.store.total_blocks();
        self.superblock.bitmap_start = crate::superblock::BITMAP_START;
        self.superblock.bitmap_blocks = self.store.bitmap_blocks()?;
        self.superblock.document_tree = document_tree;
        self.superblock.revision_tree = revision_tree;
        self.superblock.tag_tree = tag_tree;
        self.superblock.snapshot_tree = snapshot_tree;
        self.superblock.next_snapshot = self.next_snapshot;
        self.superblock.next_tag = self.next_tag;
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
