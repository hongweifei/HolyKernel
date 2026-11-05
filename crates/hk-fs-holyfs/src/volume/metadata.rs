//! 集合项、路径解析与文档视图。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::ids::DocId;
use crate::types::{
    DocumentInfo, DocumentKind, DocumentRecord, EntryInfo, EntryRecord, PayloadKind, RevisionInfo,
    RevisionRecord, validate_name,
};

use super::{MAX_REVISIONS, Volume};

impl<D: BlockDevice> Volume<D> {
    pub(crate) fn lookup(&self, parent: DocId, name: &str) -> Result<DocId, HolyError> {
        let head = self.head(parent)?;
        if !head.payload_has_collection() {
            return Err(HolyError::NotDirectory);
        }
        let entries = self.collections.get(&head.id).ok_or(HolyError::Corrupt)?;
        let entry = entries.get(name.as_bytes()).ok_or(HolyError::NotFound)?;
        Ok(entry.document)
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
}
