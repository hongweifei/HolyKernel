//! 文档与卷级令牌会话：在令牌表上补齐过期代际与跨会话转移。

use alloc::collections::BTreeMap;

use crate::error::Error;
use crate::ids::{DocId, HandleRights, RevisionId, VolumeId};
use crate::token::{Token, TokenTable, VolumeTokenTable};

/// 单个客户端的文档令牌会话。
pub struct DocSession {
    table: TokenTable,
    stale_before: u64,
}

impl DocSession {
    /// 构造空会话。
    pub const fn new() -> Self {
        Self {
            table: TokenTable::new(),
            stale_before: 0,
        }
    }

    /// 签发文档令牌。
    pub fn acquire(&mut self, document: DocId, revision: RevisionId, rights: HandleRights) -> u64 {
        self.table.acquire(document, revision, rights)
    }

    /// 查询令牌；清除后签发过的旧编号返回 `Stale`。
    pub fn get(&self, token: u64) -> Result<Token, Error> {
        self.table.get(token).ok_or(if token < self.stale_before {
            Error::Stale
        } else {
            Error::NotFound
        })
    }

    /// 更新令牌固定的修订。
    pub fn set_revision(&mut self, token: u64, revision: RevisionId) {
        self.table.set_revision(token, revision);
    }

    /// 撤销令牌及其子令牌。
    pub fn release(&mut self, token: u64) -> bool {
        self.table.revoke(token)
    }

    /// 收窄权利并签发子令牌。
    pub fn grant(&mut self, token: u64, rights: HandleRights) -> Result<u64, Error> {
        self.table.grant(token, rights)
    }

    /// 取出令牌；用于跨会话转移。
    pub fn take(&mut self, token: u64) -> Option<Token> {
        self.table.take(token)
    }

    /// 插入令牌并分配新编号。
    pub fn insert(&mut self, token: Token) -> u64 {
        self.table
            .insert(token.document, token.revision, token.rights)
    }

    /// 使全部已签发令牌过期，并保留编号序列。
    pub fn invalidate(&mut self) {
        self.stale_before = self.table.next_id();
        self.table.clear();
    }
}

impl Default for DocSession {
    fn default() -> Self {
        Self::new()
    }
}

/// 按 `(发送者, 盘符)` 分组的文档令牌会话。
pub struct DocSessions {
    documents: BTreeMap<(u64, u8), DocSession>,
}

impl DocSessions {
    /// 构造空会话表。
    pub const fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
        }
    }

    /// 返回指定发送者与盘符的会话。
    pub fn doc(&mut self, sender: u64, drive: u8) -> &mut DocSession {
        self.documents.entry((sender, drive)).or_default()
    }

    /// 在同一盘符内把令牌转移给另一个发送者。
    pub fn transfer(&mut self, from: u64, to: u64, token: u64, drive: u8) -> Result<u64, Error> {
        if from == to {
            return Err(Error::Invalid);
        }
        let entry = {
            let source = self
                .documents
                .get_mut(&(from, drive))
                .ok_or(Error::NotFound)?;
            let entry = source.get(token)?;
            if !entry.rights.contains(HandleRights::TRANSFER) {
                return Err(Error::Refused);
            }
            source.take(token).ok_or(Error::NotFound)?
        };
        Ok(self.documents.entry((to, drive)).or_default().insert(entry))
    }

    /// 使某个盘符下全部会话的文档令牌过期。
    pub fn invalidate_drive(&mut self, drive: u8) {
        for ((_, session_drive), session) in self.documents.iter_mut() {
            if *session_drive == drive {
                session.invalidate();
            }
        }
    }
}

impl Default for DocSessions {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个客户端的卷级令牌会话。
pub struct VolumeSession {
    table: VolumeTokenTable,
}

impl VolumeSession {
    /// 构造空会话。
    pub const fn new() -> Self {
        Self {
            table: VolumeTokenTable::new(),
        }
    }

    /// 签发卷级令牌。
    pub fn acquire(&mut self, volume: VolumeId, generation: u64, rights: HandleRights) -> u64 {
        self.table.acquire(volume, generation, rights)
    }

    /// 校验令牌、卷身份与卷代。
    pub fn check(
        &self,
        token: u64,
        volume: VolumeId,
        generation: u64,
        rights: HandleRights,
    ) -> Result<(), Error> {
        let entry = self.table.get(token, volume, generation)?;
        if !entry.rights.contains(rights) {
            return Err(Error::Refused);
        }
        Ok(())
    }

    /// 收窄权利并签发同一卷代下的子令牌。
    pub fn grant(
        &mut self,
        token: u64,
        rights: HandleRights,
        volume: VolumeId,
        generation: u64,
    ) -> Result<u64, Error> {
        self.table.grant(token, rights, volume, generation)
    }

    /// 撤销卷级令牌。
    pub fn release(&mut self, token: u64, volume: VolumeId, generation: u64) -> Result<(), Error> {
        self.table.get(token, volume, generation)?;
        if self.table.revoke(token) {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }
}

impl Default for VolumeSession {
    fn default() -> Self {
        Self::new()
    }
}

/// 按 `(发送者, 盘符)` 分组的卷级会话。
pub struct VolumeSessions {
    volumes: BTreeMap<(u64, u8), VolumeSession>,
}

impl VolumeSessions {
    /// 构造空会话表。
    pub const fn new() -> Self {
        Self {
            volumes: BTreeMap::new(),
        }
    }

    /// 返回指定发送者与盘符的卷级会话。
    pub fn volume(&mut self, sender: u64, drive: u8) -> &mut VolumeSession {
        self.volumes.entry((sender, drive)).or_default()
    }
}

impl Default for VolumeSessions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> DocId {
        DocId::new(7)
    }

    #[test]
    fn invalidate_keeps_old_tokens_stale() {
        let mut session = DocSession::new();
        let token = session.acquire(
            document(),
            RevisionId::ROOT,
            HandleRights::READ | HandleRights::ADMIN,
        );
        session.invalidate();
        assert_eq!(session.get(token), Err(Error::Stale));
        let next = session.acquire(document(), RevisionId::ROOT, HandleRights::READ);
        assert!(next > token);
        assert!(session.get(next).is_ok());
    }

    #[test]
    fn transfers_require_transfer_right() {
        let mut sessions = DocSessions::new();
        let token = sessions
            .doc(1, b'H')
            .acquire(document(), RevisionId::ROOT, HandleRights::READ);
        assert_eq!(sessions.transfer(1, 2, token, b'H'), Err(Error::Refused));

        let granted = sessions.doc(1, b'H').acquire(
            document(),
            RevisionId::ROOT,
            HandleRights::READ | HandleRights::TRANSFER,
        );
        let moved = sessions.transfer(1, 2, granted, b'H').expect("转移");
        assert!(sessions.doc(2, b'H').get(moved).is_ok());
        assert_eq!(sessions.doc(1, b'H').get(granted), Err(Error::NotFound));
    }

    #[test]
    fn transfers_reject_same_sender() {
        let mut sessions = DocSessions::new();
        let token =
            sessions
                .doc(1, b'H')
                .acquire(document(), RevisionId::ROOT, HandleRights::TRANSFER);
        assert_eq!(sessions.transfer(1, 1, token, b'H'), Err(Error::Invalid));
    }

    #[test]
    fn volume_tokens_expire_with_generation() {
        let volume = VolumeId::new(11);
        let mut sessions = VolumeSessions::new();
        let token = sessions
            .volume(1, b'H')
            .acquire(volume, 3, HandleRights::ADMIN);
        sessions
            .volume(1, b'H')
            .check(token, volume, 3, HandleRights::ADMIN)
            .expect("有效");
        assert_eq!(
            sessions
                .volume(1, b'H')
                .check(token, volume, 4, HandleRights::ADMIN),
            Err(Error::Stale)
        );
        assert_eq!(
            sessions
                .volume(1, b'H')
                .check(token, volume, 3, HandleRights::WRITE),
            Err(Error::Refused)
        );
    }
}
