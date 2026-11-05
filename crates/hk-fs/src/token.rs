//! 文档令牌表：权利收窄、撤销与转移。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::error::Error;
use crate::ids::{DocId, HandleRights, RevisionId};

/// 文档令牌。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// 目标文档。
    pub document: DocId,
    /// 固定修订。
    pub revision: RevisionId,
    /// 权利。
    pub rights: HandleRights,
    parent: Option<u64>,
}

/// 按会话维护的令牌表。
pub struct TokenTable {
    tokens: BTreeMap<u64, Token>,
    next: u64,
}

impl TokenTable {
    /// 构造空令牌表。
    pub const fn new() -> Self {
        Self {
            tokens: BTreeMap::new(),
            next: 1,
        }
    }

    /// 签发令牌。
    pub fn acquire(&mut self, document: DocId, revision: RevisionId, rights: HandleRights) -> u64 {
        let token = self.next;
        self.next = self.next.saturating_add(1);
        self.tokens.insert(
            token,
            Token {
                document,
                revision,
                rights,
                parent: None,
            },
        );
        token
    }

    /// 查询令牌。
    pub fn get(&self, token: u64) -> Option<Token> {
        self.tokens.get(&token).copied()
    }

    /// 更新令牌的固定修订。
    pub fn set_revision(&mut self, token: u64, revision: RevisionId) {
        if let Some(entry) = self.tokens.get_mut(&token) {
            entry.revision = revision;
        }
    }

    /// 收窄权利，签发子令牌。
    pub fn grant(&mut self, token: u64, rights: HandleRights) -> Result<u64, Error> {
        let parent = self.tokens.get(&token).ok_or(Error::NotFound)?;
        if !parent.rights.contains(rights) {
            return Err(Error::Refused);
        }
        let child = self.next;
        self.next = self.next.saturating_add(1);
        self.tokens.insert(
            child,
            Token {
                document: parent.document,
                revision: parent.revision,
                rights,
                parent: Some(token),
            },
        );
        Ok(child)
    }

    /// 撤销令牌及其全部子令牌。
    pub fn revoke(&mut self, token: u64) -> bool {
        if self.tokens.remove(&token).is_none() {
            return false;
        }
        let mut removed = alloc::vec![token];
        loop {
            let children: Vec<u64> = self
                .tokens
                .iter()
                .filter(|(_, entry)| entry.parent.is_some_and(|parent| removed.contains(&parent)))
                .map(|(id, _)| *id)
                .collect();
            if children.is_empty() {
                break;
            }
            for child in children {
                self.tokens.remove(&child);
                removed.push(child);
            }
        }
        true
    }

    /// 取出令牌；用于跨会话转移。
    pub fn take(&mut self, token: u64) -> Option<Token> {
        self.tokens.remove(&token)
    }

    /// 插入令牌并分配新编号；用于跨会话转移。
    pub fn insert(&mut self, document: DocId, revision: RevisionId, rights: HandleRights) -> u64 {
        self.acquire(document, revision, rights)
    }

    /// 返回令牌数量。
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// 判断是否为空。
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

impl Default for TokenTable {
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
    fn grants_narrower_rights_only() {
        let mut table = TokenTable::new();
        let token = table.acquire(
            document(),
            RevisionId::new(3),
            HandleRights::READ | HandleRights::INSPECT,
        );
        let read = table.grant(token, HandleRights::READ).expect("收窄");
        assert_eq!(table.get(read).expect("子令牌").rights, HandleRights::READ);
        assert_eq!(table.grant(token, HandleRights::WRITE), Err(Error::Refused));
    }

    #[test]
    fn revoke_removes_descendants() {
        let mut table = TokenTable::new();
        let token = table.acquire(
            document(),
            RevisionId::ROOT,
            HandleRights::READ | HandleRights::ADMIN,
        );
        let child = table.grant(token, HandleRights::READ).expect("子令牌");
        let grandchild = table.grant(child, HandleRights::READ).expect("孙令牌");
        assert!(table.revoke(child));
        assert!(table.get(child).is_none());
        assert!(table.get(grandchild).is_none());
        assert!(table.get(token).is_some());
        assert!(!table.revoke(child));
    }

    #[test]
    fn transfers_between_tables() {
        let mut source = TokenTable::new();
        let token = source.acquire(
            document(),
            RevisionId::new(9),
            HandleRights::READ | HandleRights::TRANSFER,
        );
        let entry = source.take(token).expect("取出");
        let mut target = TokenTable::new();
        let moved = target.insert(entry.document, entry.revision, entry.rights);
        assert!(source.get(token).is_none());
        assert_eq!(
            target.get(moved).expect("目标令牌").revision,
            RevisionId::new(9)
        );
    }
}
