//! 流式写 staging：先累积内容，Commit 时生成正式修订。

use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::ids::{DocId, RevisionId};
use crate::types::DocumentKind;

use super::{MAX_CONTENT_BYTES, Volume};

pub(super) struct Staging {
    pub(super) document: DocId,
    pub(super) base: RevisionId,
    pub(super) content: Vec<u8>,
}

impl<D: BlockDevice> Volume<D> {
    /// 创建 staging，固定当前 head 作为提交基线。
    pub(crate) fn stage(
        &mut self,
        document: DocId,
        expected: Option<RevisionId>,
    ) -> Result<u64, HolyError> {
        self.ensure_writable()?;
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        if expected.is_some_and(|expected| expected != record.head) {
            return Err(HolyError::Stale);
        }
        let base = record.head;
        let content = self.read_document(document)?;
        let id = self.next_staging;
        self.next_staging = self.next_staging.saturating_add(1);
        self.staging.insert(
            id,
            Staging {
                document,
                base,
                content,
            },
        );
        Ok(id)
    }

    /// 写入 staging 区间。
    pub(crate) fn write_staging(
        &mut self,
        staging: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, HolyError> {
        self.ensure_writable()?;
        let entry = self.staging.get_mut(&staging).ok_or(HolyError::NotFound)?;
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(HolyError::Invalid)?;
        if end > MAX_CONTENT_BYTES {
            return Err(HolyError::NoSpace);
        }
        if end as usize > entry.content.len() {
            entry.content.resize(end as usize, 0);
        }
        entry.content[offset as usize..end as usize].copy_from_slice(data);
        Ok(data.len())
    }

    /// 提交 staging，返回新 head 修订。
    pub(crate) fn commit_staging(
        &mut self,
        staging: u64,
        expected: Option<RevisionId>,
    ) -> Result<RevisionId, HolyError> {
        self.ensure_writable()?;
        let result = self.commit_staging_inner(staging, expected);
        if let Err(error) = &result {
            self.note_error(error);
        }
        result
    }

    fn commit_staging_inner(
        &mut self,
        staging: u64,
        expected: Option<RevisionId>,
    ) -> Result<RevisionId, HolyError> {
        let entry = self.staging.get(&staging).ok_or(HolyError::NotFound)?;
        let document = entry.document;
        let base = entry.base;
        let content = entry.content.clone();
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.head != base || expected.is_some_and(|expected| expected != record.head) {
            return Err(HolyError::Stale);
        }
        let head = *self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        self.check_quota(0, (content.len() as u64).saturating_sub(head.size), 1, 0, 0)?;
        self.replace_content(document, &head, &content)?;
        self.staging.remove(&staging);
        let head = self
            .documents
            .get(&document)
            .ok_or(HolyError::Corrupt)?
            .head;
        Ok(head)
    }

    /// 放弃 staging。
    pub(crate) fn abort_staging(&mut self, staging: u64) -> Result<(), HolyError> {
        self.staging
            .remove(&staging)
            .map(|_| ())
            .ok_or(HolyError::NotFound)
    }
}
