//! 文档内容读写、截断与内容修订生成。

use alloc::vec;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::compress::{compress, decompress};
use crate::error::HolyError;
use crate::hash::content_hash;
use crate::ids::{DocId, RevisionId};
use crate::superblock::BLOCK_SIZE;
use crate::types::{DocumentKind, Extent, MAX_EXTENTS, PayloadKind, RevisionFlags, RevisionRecord};

use super::{MAX_CONTENT_BYTES, Volume};

#[derive(Clone)]
pub(super) struct DedupEntry {
    pub(super) extents: Vec<Extent>,
    pub(super) compressed: bool,
}

impl<D: BlockDevice> Volume<D> {
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

    pub(crate) fn read_range_at(
        &self,
        document: DocId,
        revision: RevisionId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let revision = self.revisions.get(&revision).ok_or(HolyError::NotFound)?;
        if revision.document != document {
            return Err(HolyError::NotFound);
        }
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
        let new_size = head.size.max(end);
        self.check_quota(0, new_size.saturating_sub(head.size), 1, 0, 0)?;
        if head.size > 0 && offset == head.size && head.size.is_multiple_of(BLOCK_SIZE as u64) {
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
        self.check_quota(0, size.saturating_sub(head.size), 1, 0, 0)?;
        let mut content = vec![0_u8; size as usize];
        let copy = head.size.min(size) as usize;
        if copy > 0 {
            self.read_extents(&head, 0, &mut content[..copy])?;
        }
        self.replace_content(document, &head, &content)
    }

    /// 整文档读取。
    pub(crate) fn read_document(&self, document: DocId) -> Result<Vec<u8>, HolyError> {
        let record = self.documents.get(&document).ok_or(HolyError::NotFound)?;
        if record.kind == DocumentKind::Collection {
            return Err(HolyError::IsDirectory);
        }
        let revision = self.revisions.get(&record.head).ok_or(HolyError::Corrupt)?;
        let buffer = self.read_content(revision)?;
        if content_hash(&buffer) != revision.content_hash {
            return Err(HolyError::Damaged);
        }
        Ok(buffer)
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
        self.check_quota(0, (content.len() as u64).saturating_sub(head.size), 1, 0, 0)?;
        self.replace_content(document, &head, content)?;
        let record = self.documents.get(&document).ok_or(HolyError::Corrupt)?;
        Ok(record.head)
    }

    fn read_extents(
        &self,
        revision: &RevisionRecord,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<(), HolyError> {
        if revision.flags.contains(RevisionFlags::COMPRESSED) {
            let content = self.read_content(revision)?;
            let start = offset as usize;
            let end = start.checked_add(buffer.len()).ok_or(HolyError::Corrupt)?;
            let content = content.get(start..end).ok_or(HolyError::Corrupt)?;
            buffer.copy_from_slice(content);
            return Ok(());
        }
        self.read_raw_extents(revision, offset, buffer)
    }

    fn read_raw_extents(
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

    fn read_content(&self, revision: &RevisionRecord) -> Result<Vec<u8>, HolyError> {
        let size = revision.size as usize;
        if revision.flags.contains(RevisionFlags::COMPRESSED) {
            let capacity: usize = revision
                .extents()
                .iter()
                .map(|extent| extent.bytes() as usize)
                .sum();
            if capacity == 0 {
                return Err(HolyError::Corrupt);
            }
            let mut payload = vec![0_u8; capacity];
            self.read_raw_extents(revision, 0, &mut payload)?;
            return decompress(&payload, size);
        }
        let mut content = vec![0_u8; size];
        if size > 0 {
            self.read_raw_extents(revision, 0, &mut content)?;
        }
        Ok(content)
    }

    /// 读取指定修订的完整内容，不校验哈希；供维护扫描使用。
    pub(super) fn revision_content(&self, revision: &RevisionRecord) -> Result<Vec<u8>, HolyError> {
        self.read_content(revision)
    }

    fn append_hash(&self, head: &RevisionRecord, data: &[u8]) -> Result<u64, HolyError> {
        if head.size == 0 {
            return Ok(content_hash(data));
        }
        let mut content = vec![0_u8; head.size as usize];
        self.read_extents(head, 0, &mut content)?;
        content.extend_from_slice(data);
        Ok(content_hash(&content))
    }

    fn append_content(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        offset: u64,
        data: &[u8],
    ) -> Result<(), HolyError> {
        if head.flags.contains(RevisionFlags::COMPRESSED) {
            return self.rewrite_content(document, head, offset, data);
        }
        let hash = self.append_hash(head, data)?;
        let size = offset + data.len() as u64;
        if let Some(entry) = self.dedup_lookup(hash, size) {
            self.dedup_hits = self.dedup_hits.saturating_add(1);
            return self.insert_content_revision(
                document,
                head,
                entry.extents,
                size,
                hash,
                entry.compressed,
            );
        }
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
        self.dedup_insert(hash, size, &extents, false);
        self.insert_content_revision(document, head, extents, size, hash, false)
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

    pub(super) fn replace_content(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        content: &[u8],
    ) -> Result<(), HolyError> {
        let hash = content_hash(content);
        let size = content.len() as u64;
        if let Some(entry) = self.dedup_lookup(hash, size) {
            self.dedup_hits = self.dedup_hits.saturating_add(1);
            return self.insert_content_revision(
                document,
                head,
                entry.extents,
                size,
                hash,
                entry.compressed,
            );
        }
        let (payload, compressed) = match compress(content) {
            Some(payload) => (payload, true),
            None => (content.to_vec(), false),
        };
        let blocks = payload.len().div_ceil(BLOCK_SIZE) as u64;
        let extents = if blocks == 0 {
            Vec::new()
        } else {
            let start = self.store.allocate_blocks(blocks)?;
            self.store
                .write_bytes(start * BLOCK_SIZE as u64, &payload)?;
            vec![Extent {
                start,
                blocks: blocks as u32,
            }]
        };
        self.dedup_insert(hash, size, &extents, compressed);
        self.insert_content_revision(document, head, extents, size, hash, compressed)
    }

    fn dedup_lookup(&self, hash: u64, size: u64) -> Option<DedupEntry> {
        if size == 0 {
            return None;
        }
        let entry = self.dedup.get(&(hash, size))?;
        (entry.extents.len() <= MAX_EXTENTS).then(|| entry.clone())
    }

    fn dedup_insert(&mut self, hash: u64, size: u64, extents: &[Extent], compressed: bool) {
        if size == 0 || extents.is_empty() {
            return;
        }
        self.dedup
            .entry((hash, size))
            .or_insert_with(|| DedupEntry {
                extents: extents.to_vec(),
                compressed,
            });
    }

    /// 从当前修订集重建去重索引；GC 或裁剪后调用。
    pub(super) fn rebuild_dedup(&mut self) {
        self.dedup.clear();
        for revision in self.revisions.values() {
            if revision.size == 0
                || !matches!(revision.payload, PayloadKind::Content | PayloadKind::Both)
                || revision.extents().len() > MAX_EXTENTS
            {
                continue;
            }
            self.dedup
                .entry((revision.content_hash, revision.size))
                .or_insert_with(|| DedupEntry {
                    extents: revision.extents().to_vec(),
                    compressed: revision.flags.contains(RevisionFlags::COMPRESSED),
                });
        }
    }

    fn insert_content_revision(
        &mut self,
        document: DocId,
        head: &RevisionRecord,
        extents: Vec<Extent>,
        size: u64,
        content_hash: u64,
        compressed: bool,
    ) -> Result<(), HolyError> {
        let revision = self.allocate_revision();
        let mut record = RevisionRecord::empty(revision, document, Some(head.id));
        record.extent_count = extents.len() as u16;
        for (slot, extent) in record.extents.iter_mut().zip(extents) {
            *slot = extent;
        }
        record.size = size;
        record.content_hash = content_hash;
        if compressed {
            record.flags =
                RevisionFlags::from_bits(record.flags.bits() | RevisionFlags::COMPRESSED.bits());
        }
        self.revisions.insert(revision, record);
        if let Some(document) = self.documents.get_mut(&document) {
            document.head = revision;
        }
        self.dirty = true;
        Ok(())
    }
}
