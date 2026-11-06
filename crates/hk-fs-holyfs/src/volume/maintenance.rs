//! 卷完整性与维护：Check、Scrub 与 Repair。

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::hash::content_hash;
use crate::superblock::BLOCK_SIZE;
use crate::tree::collect_blocks;
use crate::types::{DocumentKind, MaintenanceReport, PayloadKind, validate_name};

use super::Volume;

impl<D: BlockDevice> Volume<D> {
    /// 只读校验对象图、引用与内容哈希。
    pub(crate) fn check(&self) -> Result<MaintenanceReport, HolyError> {
        let (report, _, missing) = self.scan()?;
        if missing {
            return Err(HolyError::Corrupt);
        }
        Ok(report)
    }

    /// 校验并回收不可达的泄漏块。
    pub(crate) fn scrub(&mut self) -> Result<MaintenanceReport, HolyError> {
        let (mut report, reachable, missing) = self.scan()?;
        if missing {
            return Err(HolyError::Corrupt);
        }
        report.repaired_blocks = self.release_leaks(&reachable)?;
        self.rebuild_dedup();
        Ok(report)
    }

    /// 校验并修复位图：补记可达块、回收泄漏块。
    pub(crate) fn repair(&mut self) -> Result<MaintenanceReport, HolyError> {
        let (mut report, reachable, _) = self.scan()?;
        let mut repaired = 0;
        for block in &reachable {
            if !self.store.block_allocated(*block) {
                self.store.set_allocated(*block, true);
                repaired += 1;
            }
        }
        repaired += self.release_leaks(&reachable)?;
        if repaired > 0 {
            self.store.write_bitmap()?;
            self.store.flush()?;
        }
        self.rebuild_dedup();
        report.repaired_blocks = repaired;
        Ok(report)
    }

    #[cfg(test)]
    pub(crate) fn allocate_leak(&mut self, count: u64) -> Result<u64, HolyError> {
        self.store.allocate_blocks(count)
    }

    fn release_leaks(&mut self, keep: &BTreeSet<u64>) -> Result<u64, HolyError> {
        let mut released = 0;
        for block in self.store.data_start()..self.store.total_blocks() {
            if self.store.block_allocated(block) && !keep.contains(&block) {
                self.store.set_allocated(block, false);
                released += 1;
            }
        }
        if released > 0 {
            self.store.write_bitmap()?;
            self.store.flush()?;
        }
        Ok(released)
    }

    fn scan(&self) -> Result<(MaintenanceReport, BTreeSet<u64>, bool), HolyError> {
        let mut report = MaintenanceReport {
            documents: self.documents.len() as u64,
            revisions: self.revisions.len() as u64,
            collections: self.collections.len() as u64,
            tags: self.tags.len() as u64,
            snapshots: self.snapshots.len() as u64,
            ..MaintenanceReport::default()
        };
        let mut reachable = BTreeSet::new();
        self.reach_tree(self.superblock.document_tree, &mut reachable)?;
        self.reach_tree(self.superblock.revision_tree, &mut reachable)?;
        for root in [self.superblock.tag_tree, self.superblock.snapshot_tree] {
            if root != 0 {
                self.reach_tree(root, &mut reachable)?;
            }
        }
        for revision in self.revisions.values() {
            if revision.entries_root != 0 {
                self.reach_tree(revision.entries_root, &mut reachable)?;
            }
            for extent in revision.extents() {
                if extent.blocks == 0
                    || extent.start < self.store.data_start()
                    || extent.end() > self.store.total_blocks()
                {
                    return Err(HolyError::Corrupt);
                }
                for block in extent.start..extent.end() {
                    reachable.insert(block);
                }
            }
        }

        let root = self
            .documents
            .get(&crate::ids::DocId::ROOT)
            .ok_or(HolyError::Corrupt)?;
        if root.kind != DocumentKind::Collection || !self.revision_matches(root.head, root.document)
        {
            return Err(HolyError::Corrupt);
        }
        for (document, record) in &self.documents {
            if record.document != *document {
                return Err(HolyError::Corrupt);
            }
            if !self.revision_matches(record.head, record.document) {
                return Err(HolyError::Corrupt);
            }
            let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
            if record.kind == DocumentKind::Collection && !revision.payload_has_collection() {
                return Err(HolyError::Corrupt);
            }
        }
        for (revision_id, revision) in &self.revisions {
            if revision.id != *revision_id {
                return Err(HolyError::Corrupt);
            }
            if !self.documents.contains_key(&revision.document) {
                return Err(HolyError::Corrupt);
            }
            if let Some(parent) = revision.parent {
                let parent = self.revisions.get(&parent).ok_or(HolyError::Corrupt)?;
                if parent.document != revision.document {
                    return Err(HolyError::Corrupt);
                }
            }
            if revision.payload_has_collection() && !self.collections.contains_key(&revision.id) {
                return Err(HolyError::Corrupt);
            }
            self.validate_content(revision)?;
            report.content_bytes = report.content_bytes.saturating_add(revision.size);
        }
        for (revision_id, entries) in &self.collections {
            let revision = self.revisions.get(revision_id).ok_or(HolyError::Corrupt)?;
            if !revision.payload_has_collection() {
                return Err(HolyError::Corrupt);
            }
            for (name, entry) in entries {
                let name = core::str::from_utf8(name).map_err(|_| HolyError::Corrupt)?;
                validate_name(name)?;
                let target = self
                    .documents
                    .get(&entry.document)
                    .ok_or(HolyError::Corrupt)?;
                if target.kind != entry.kind {
                    return Err(HolyError::Corrupt);
                }
                report.entries = report.entries.saturating_add(1);
            }
        }
        for name in self.tags.values() {
            validate_name(name)?;
        }
        for (tag, members) in &self.tag_members {
            if !self.tags.contains_key(tag) {
                return Err(HolyError::Corrupt);
            }
            for document in members {
                if !self.documents.contains_key(document) {
                    return Err(HolyError::Corrupt);
                }
            }
        }
        for record in self.snapshots.values() {
            let root = record
                .documents
                .get(&record.root_document)
                .ok_or(HolyError::Corrupt)?;
            if root.head != record.root_revision {
                return Err(HolyError::Corrupt);
            }
            for record in record.documents.values() {
                if !self.revision_matches(record.head, record.document) {
                    return Err(HolyError::Corrupt);
                }
            }
            for (tag, members) in &record.tag_members {
                if !record.tags.contains_key(tag) {
                    return Err(HolyError::Corrupt);
                }
                for document in members {
                    if !record.documents.contains_key(document) {
                        return Err(HolyError::Corrupt);
                    }
                }
            }
        }

        let mut missing = false;
        for block in &reachable {
            if !self.store.block_allocated(*block) {
                missing = true;
            }
        }
        for block in self.store.data_start()..self.store.total_blocks() {
            if self.store.block_allocated(block) && !reachable.contains(&block) {
                report.leaked_blocks = report.leaked_blocks.saturating_add(1);
            }
        }
        Ok((report, reachable, missing))
    }

    fn revision_matches(
        &self,
        revision: crate::ids::RevisionId,
        document: crate::ids::DocId,
    ) -> bool {
        self.revisions
            .get(&revision)
            .is_some_and(|record| record.document == document)
    }

    fn validate_content(&self, revision: &crate::types::RevisionRecord) -> Result<(), HolyError> {
        if !matches!(revision.payload, PayloadKind::Content | PayloadKind::Both) {
            return Ok(());
        }
        let mut capacity = 0_u64;
        let mut previous_end = 0_u64;
        for extent in revision.extents() {
            if extent.blocks == 0
                || extent.start < self.store.data_start()
                || extent.end() > self.store.total_blocks()
                || (previous_end != 0 && extent.start < previous_end)
            {
                return Err(HolyError::Corrupt);
            }
            previous_end = extent.end();
            capacity = capacity.saturating_add(extent.bytes());
        }
        if revision.size > capacity {
            return Err(HolyError::Corrupt);
        }
        if capacity > 0 && capacity - revision.size >= BLOCK_SIZE as u64 {
            return Err(HolyError::Corrupt);
        }
        let content = self.revision_content(revision)?;
        if content_hash(&content) != revision.content_hash {
            return Err(HolyError::Damaged);
        }
        Ok(())
    }

    fn reach_tree(&self, root: u64, reachable: &mut BTreeSet<u64>) -> Result<(), HolyError> {
        let mut blocks = Vec::new();
        collect_blocks(&self.store, root, &mut blocks)?;
        reachable.extend(blocks);
        Ok(())
    }
}
