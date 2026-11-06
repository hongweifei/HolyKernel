//! 卷配额：文档数、逻辑字节、修订、快照与标签。

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::types::Quota;

use super::Volume;

impl<D: BlockDevice> Volume<D> {
    /// 返回当前配额。
    pub(crate) fn quota(&self) -> Quota {
        Quota {
            documents: self.superblock.quota_documents,
            bytes: self.superblock.quota_bytes,
            revisions: self.superblock.quota_revisions,
            snapshots: self.superblock.quota_snapshots,
            tags: self.superblock.quota_tags,
        }
    }

    /// 更新配额并提交。
    pub(crate) fn set_quota(&mut self, quota: Quota) -> Result<(), HolyError> {
        self.superblock.quota_documents = quota.documents;
        self.superblock.quota_bytes = quota.bytes;
        self.superblock.quota_revisions = quota.revisions;
        self.superblock.quota_snapshots = quota.snapshots;
        self.superblock.quota_tags = quota.tags;
        self.dirty = true;
        self.persist()
    }

    /// 返回当前使用量：文档、逻辑字节、修订、快照、标签。
    pub(crate) fn quota_used(&self) -> Quota {
        let bytes = self
            .documents
            .values()
            .filter_map(|record| self.revisions.get(&record.head))
            .filter(|record| !record.payload_has_collection())
            .map(|record| record.size)
            .sum();
        Quota {
            documents: self.documents.len() as u64,
            bytes,
            revisions: self.revisions.len() as u64,
            snapshots: self.snapshots.len() as u64,
            tags: self.tags.len() as u64,
        }
    }

    /// 检查增量是否越过配额。
    pub(super) fn check_quota(
        &self,
        documents: u64,
        bytes: u64,
        revisions: u64,
        snapshots: u64,
        tags: u64,
    ) -> Result<(), HolyError> {
        let used = self.quota_used();
        let quota = self.quota();
        exceeds(usage(used.documents, documents), quota.documents)
            .then_some(HolyError::Quota)
            .map_or(Ok(()), Err)?;
        exceeds(usage(used.bytes, bytes), quota.bytes)
            .then_some(HolyError::Quota)
            .map_or(Ok(()), Err)?;
        exceeds(usage(used.revisions, revisions), quota.revisions)
            .then_some(HolyError::Quota)
            .map_or(Ok(()), Err)?;
        exceeds(usage(used.snapshots, snapshots), quota.snapshots)
            .then_some(HolyError::Quota)
            .map_or(Ok(()), Err)?;
        exceeds(usage(used.tags, tags), quota.tags)
            .then_some(HolyError::Quota)
            .map_or(Ok(()), Err)
    }
}

fn usage(current: u64, delta: u64) -> u64 {
    current.saturating_add(delta)
}

fn exceeds(value: u64, limit: u64) -> bool {
    limit != 0 && value > limit
}
