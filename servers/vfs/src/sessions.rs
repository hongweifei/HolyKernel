//! 原生 VFS 文档令牌会话。

use alloc::collections::BTreeMap;

use hk_fs::{DocId, HandleRights, RevisionId, Token, TokenTable};
use hk_proto::vfs::{STATUS_NOT_FOUND, STATUS_STALE};

/// 文档令牌表。
#[cfg(target_os = "none")]
pub(crate) struct DocSession {
    table: TokenTable,
    stale_before: u64,
}

#[cfg(target_os = "none")]
impl DocSession {
    const fn new() -> Self {
        Self {
            table: TokenTable::new(),
            stale_before: 0,
        }
    }

    pub(crate) fn acquire(&mut self, document: DocId, revision: RevisionId, rights: u32) -> u64 {
        self.table
            .acquire(document, revision, HandleRights::from_bits(rights))
    }

    pub(crate) fn get(&self, token: u64) -> Result<Token, u8> {
        match self.table.get(token) {
            Some(entry) => Ok(entry),
            None if token < self.stale_before => Err(STATUS_STALE),
            None => Err(STATUS_NOT_FOUND),
        }
    }

    pub(crate) fn set_revision(&mut self, token: u64, revision: RevisionId) {
        self.table.set_revision(token, revision);
    }

    pub(crate) fn release(&mut self, token: u64) -> bool {
        self.table.revoke(token)
    }

    pub(crate) fn grant(&mut self, token: u64, rights: u32) -> Result<u64, hk_fs::Error> {
        self.table.grant(token, HandleRights::from_bits(rights))
    }

    pub(crate) fn take(&mut self, token: u64) -> Option<Token> {
        self.table.take(token)
    }

    pub(crate) fn insert(&mut self, token: Token) -> u64 {
        self.table
            .insert(token.document, token.revision, token.rights)
    }
}

#[cfg(target_os = "none")]
pub(crate) struct Sessions {
    documents: BTreeMap<(u64, u8), DocSession>,
}

#[cfg(target_os = "none")]
impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
        }
    }

    pub(crate) fn doc(&mut self, sender: u64, drive: u8) -> &mut DocSession {
        self.documents
            .entry((sender, drive))
            .or_insert_with(DocSession::new)
    }

    pub(crate) fn transfer(
        &mut self,
        from: u64,
        to: u64,
        token: u64,
        drive: u8,
    ) -> Result<u64, u8> {
        use hk_proto::vfs::{STATUS_INVALID, STATUS_NOT_FOUND, STATUS_PERMISSION};

        if from == to {
            return Err(STATUS_INVALID);
        }
        let entry = {
            let source = self
                .documents
                .get_mut(&(from, drive))
                .ok_or(STATUS_NOT_FOUND)?;
            let entry = source.get(token)?;
            if !entry.rights.contains(HandleRights::TRANSFER) {
                return Err(STATUS_PERMISSION);
            }
            source.take(token).ok_or(STATUS_NOT_FOUND)?
        };
        let target = self
            .documents
            .entry((to, drive))
            .or_insert_with(DocSession::new);
        Ok(target.insert(entry))
    }
}
