//! 修订保留、封存与显式销毁。

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::ids::{DocId, RevisionId};
use crate::types::{MaintenanceReport, RevisionFlags};

use super::{MAX_REVISIONS, Volume};

impl<D: BlockDevice> Volume<D> {
    /// 封存修订，使其不参与自动裁剪。
    pub(crate) fn seal(&mut self, revision: RevisionId) -> Result<(), HolyError> {
        self.ensure_writable()?;
        let record = self
            .revisions
            .get_mut(&revision)
            .ok_or(HolyError::NotFound)?;
        record.flags = RevisionFlags::from_bits(record.flags.bits() | RevisionFlags::SEALED.bits());
        self.dirty = true;
        self.persist()
    }

    /// 保留最近 `keep_last` 条未封存修订与全部封存/快照固定修订，随后回收旧块。
    pub(crate) fn prune(
        &mut self,
        document: DocId,
        keep_last: u64,
    ) -> Result<MaintenanceReport, HolyError> {
        self.ensure_writable()?;
        if keep_last == 0 {
            return Err(HolyError::Invalid);
        }
        let head = self
            .documents
            .get(&document)
            .ok_or(HolyError::NotFound)?
            .head;
        let pinned = self.pinned_revisions()?;
        let mut keep = BTreeSet::new();
        let mut unsealed = 0_u64;
        let mut current = Some(head);
        let mut steps = 0;
        while let Some(revision) = current {
            steps += 1;
            if steps > MAX_REVISIONS {
                return Err(HolyError::Corrupt);
            }
            let record = self.revisions.get(&revision).ok_or(HolyError::Corrupt)?;
            let sealed = record.flags.contains(RevisionFlags::SEALED);
            let fixed = pinned.contains(&revision);
            if sealed || fixed || unsealed < keep_last {
                keep.insert(revision);
                if !sealed && !fixed {
                    unsealed = unsealed.saturating_add(1);
                }
            }
            current = record.parent;
        }
        if !keep.contains(&head) {
            return Err(HolyError::Corrupt);
        }

        let removed: Vec<RevisionId> = self
            .revisions
            .iter()
            .filter(|(id, record)| record.document == document && !keep.contains(id))
            .map(|(id, _)| *id)
            .collect();
        for revision in &removed {
            self.revisions.remove(revision);
            self.collections.remove(revision);
        }
        for revision in &keep {
            if let Some(record) = self.revisions.get_mut(revision)
                && record.parent.is_some_and(|parent| !keep.contains(&parent))
            {
                record.parent = None;
            }
        }
        self.dirty = true;
        self.persist()?;
        self.scrub()
    }

    /// 显式销毁文档、当前集合项、标签关系与修订链；快照固定时返回 `Pinned`。
    pub(crate) fn purge(&mut self, document: DocId) -> Result<(), HolyError> {
        self.ensure_writable()?;
        if document == DocId::ROOT {
            return Err(HolyError::Invalid);
        }
        if !self.documents.contains_key(&document) {
            return Err(HolyError::NotFound);
        }
        if self
            .snapshots
            .values()
            .any(|snapshot| snapshot.documents.contains_key(&document))
        {
            return Err(HolyError::Pinned);
        }
        let mut parents: Vec<(DocId, String)> = Vec::new();
        for (parent, record) in &self.documents {
            let head = match self.revisions.get(&record.head) {
                Some(head) => head,
                None => continue,
            };
            let Some(entries) = self.collections.get(&head.id) else {
                continue;
            };
            for (name, entry) in entries {
                if entry.document == document {
                    let Ok(name) = core::str::from_utf8(name) else {
                        return Err(HolyError::Corrupt);
                    };
                    parents.push((*parent, String::from(name)));
                }
            }
        }
        for (parent, name) in parents {
            self.remove_entry(parent, &name)?;
        }
        for members in self.tag_members.values_mut() {
            members.remove(&document);
        }
        let stale: Vec<RevisionId> = self
            .collections
            .iter()
            .filter(|(_, entries)| entries.values().any(|entry| entry.document == document))
            .map(|(revision, _)| *revision)
            .collect();
        let revisions: Vec<RevisionId> = self
            .revisions
            .iter()
            .filter(|(id, record)| record.document == document || stale.contains(id))
            .map(|(id, _)| *id)
            .collect();
        for revision in revisions {
            self.revisions.remove(&revision);
            self.collections.remove(&revision);
        }
        let remaining: BTreeSet<RevisionId> = self.revisions.keys().copied().collect();
        for record in self.revisions.values_mut() {
            if record
                .parent
                .is_some_and(|parent| !remaining.contains(&parent))
            {
                record.parent = None;
            }
        }
        self.documents.remove(&document);
        self.dirty = true;
        self.persist()?;
        self.scrub()?;
        Ok(())
    }

    fn remove_entry(&mut self, parent: DocId, name: &str) -> Result<(), HolyError> {
        let head = *self.head(parent)?;
        if !head.payload_has_collection() {
            return Err(HolyError::NotDirectory);
        }
        let mut entries = self
            .collections
            .get(&head.id)
            .cloned()
            .ok_or(HolyError::Corrupt)?;
        if entries.remove(name.as_bytes()).is_none() {
            return Err(HolyError::NotFound);
        }
        let revision = self.allocate_revision();
        let mut record = head;
        record.id = revision;
        record.parent = Some(head.id);
        record.entries_root = 0;
        self.revisions.insert(revision, record);
        self.collections.insert(revision, entries);
        if let Some(document) = self.documents.get_mut(&parent) {
            document.head = revision;
        }
        Ok(())
    }

    fn pinned_revisions(&self) -> Result<BTreeSet<RevisionId>, HolyError> {
        let mut pinned = BTreeSet::new();
        for snapshot in self.snapshots.values() {
            for record in snapshot.documents.values() {
                let mut current = Some(record.head);
                let mut steps = 0;
                while let Some(revision) = current {
                    steps += 1;
                    if steps > MAX_REVISIONS {
                        return Err(HolyError::Corrupt);
                    }
                    let record = self.revisions.get(&revision).ok_or(HolyError::Corrupt)?;
                    pinned.insert(revision);
                    current = record.parent;
                }
            }
        }
        Ok(pinned)
    }
}
