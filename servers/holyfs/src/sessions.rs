//! HolyFS 服务的文档令牌与卷级令牌会话。

use alloc::collections::BTreeMap;

use hk_fs::{
    DocId, Error, HandleRights, RevisionId, Token, TokenTable, VolumeId, VolumeTokenTable,
};
use hk_server_api::vfs::{STATUS_ERROR, STATUS_NOT_FOUND, STATUS_PERMISSION, STATUS_STALE};

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

    pub(crate) fn invalidate(&mut self) {
        self.stale_before = self.table.next_id();
        self.table.clear();
    }
}

/// 卷级令牌表。
#[cfg(target_os = "none")]
pub(crate) struct VolumeSession {
    table: VolumeTokenTable,
}

#[cfg(target_os = "none")]
impl VolumeSession {
    const fn new() -> Self {
        Self {
            table: VolumeTokenTable::new(),
        }
    }

    pub(crate) fn acquire(&mut self, volume: VolumeId, epoch: u64, rights: u32) -> u64 {
        self.table
            .acquire(volume, epoch, HandleRights::from_bits(rights))
    }

    pub(crate) fn check(
        &self,
        token: u64,
        volume: VolumeId,
        epoch: u64,
        rights: u32,
    ) -> Result<(), u8> {
        let entry = self
            .table
            .get(token, volume, epoch)
            .map_err(volume_status)?;
        if !entry.rights.contains(HandleRights::from_bits(rights)) {
            return Err(STATUS_PERMISSION);
        }
        Ok(())
    }

    pub(crate) fn grant(
        &mut self,
        token: u64,
        rights: u32,
        volume: VolumeId,
        epoch: u64,
    ) -> Result<u64, u8> {
        self.table
            .grant(token, HandleRights::from_bits(rights), volume, epoch)
            .map_err(volume_status)
    }

    pub(crate) fn release(&mut self, token: u64, volume: VolumeId, epoch: u64) -> Result<(), u8> {
        self.table
            .get(token, volume, epoch)
            .map_err(volume_status)?;
        if self.table.revoke(token) {
            Ok(())
        } else {
            Err(STATUS_NOT_FOUND)
        }
    }
}

#[cfg(target_os = "none")]
fn volume_status(error: Error) -> u8 {
    match error {
        Error::NotFound => STATUS_NOT_FOUND,
        Error::Stale => STATUS_STALE,
        Error::Refused => STATUS_PERMISSION,
        _ => STATUS_ERROR,
    }
}

#[cfg(target_os = "none")]
pub(crate) struct Sessions {
    documents: BTreeMap<u64, DocSession>,
    volumes: BTreeMap<u64, VolumeSession>,
}

#[cfg(target_os = "none")]
impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
            volumes: BTreeMap::new(),
        }
    }

    pub(crate) fn doc(&mut self, sender: u64) -> &mut DocSession {
        self.documents.entry(sender).or_insert_with(DocSession::new)
    }

    pub(crate) fn volume(&mut self, sender: u64) -> &mut VolumeSession {
        self.volumes
            .entry(sender)
            .or_insert_with(VolumeSession::new)
    }

    pub(crate) fn transfer(&mut self, from: u64, to: u64, token: u64) -> Result<u64, u8> {
        use hk_server_api::vfs::{STATUS_INVALID, STATUS_NOT_FOUND, STATUS_PERMISSION};

        if from == to {
            return Err(STATUS_INVALID);
        }
        let entry = {
            let source = self.documents.get_mut(&from).ok_or(STATUS_NOT_FOUND)?;
            let entry = source.get(token)?;
            if !entry.rights.contains(HandleRights::TRANSFER) {
                return Err(STATUS_PERMISSION);
            }
            source.take(token).ok_or(STATUS_NOT_FOUND)?
        };
        let target = self.documents.entry(to).or_insert_with(DocSession::new);
        Ok(target.insert(entry))
    }

    pub(crate) fn invalidate_documents(&mut self) {
        for session in self.documents.values_mut() {
            session.invalidate();
        }
    }
}
