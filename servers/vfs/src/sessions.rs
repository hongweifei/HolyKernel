//! VFS 会话与客户端共享帧。

use alloc::collections::BTreeMap;

use hk_block::ipc::{register_client_frame, unregister_client_frame};
use hk_fs_holyfs::{DocId, RevisionId};
use hk_server_api::vfs::{VFS_SERVER_FRAME_BASE, VFS_SERVER_FRAME_STRIDE};
use hk_server_api::{close_handle, frame_map, frame_unmap};
use hk_vfs::{FileHandle, Vfs};

/// 文档令牌。
#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
pub(crate) struct Token {
    pub(crate) document: DocId,
    pub(crate) revision: RevisionId,
    pub(crate) rights: u32,
}

/// 文档令牌表。
#[cfg(target_os = "none")]
pub(crate) struct DocSession {
    tokens: BTreeMap<u64, Token>,
    next: u64,
}

#[cfg(target_os = "none")]
impl DocSession {
    const fn new() -> Self {
        Self {
            tokens: BTreeMap::new(),
            next: 1,
        }
    }

    pub(crate) fn acquire(&mut self, document: DocId, revision: RevisionId, rights: u32) -> u64 {
        let token = self.next;
        self.next = self.next.saturating_add(1);
        self.tokens.insert(
            token,
            Token {
                document,
                revision,
                rights,
            },
        );
        token
    }

    pub(crate) fn get(&self, token: u64) -> Option<Token> {
        self.tokens.get(&token).copied()
    }

    pub(crate) fn set_revision(&mut self, token: u64, revision: RevisionId) {
        if let Some(entry) = self.tokens.get_mut(&token) {
            entry.revision = revision;
        }
    }

    pub(crate) fn release(&mut self, token: u64) -> bool {
        self.tokens.remove(&token).is_some()
    }
}

#[cfg(target_os = "none")]
pub(crate) struct Session {
    handles: BTreeMap<u64, FileHandle>,
    next: u64,
}

#[cfg(target_os = "none")]
impl Session {
    const fn new() -> Self {
        Self {
            handles: BTreeMap::new(),
            next: 1,
        }
    }

    pub(crate) fn insert(&mut self, handle: FileHandle) -> u64 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.handles.insert(id, handle);
        id
    }

    pub(crate) fn get(&self, id: u64) -> Option<FileHandle> {
        self.handles.get(&id).copied()
    }

    pub(crate) fn remove(&mut self, id: u64) -> Option<FileHandle> {
        self.handles.remove(&id)
    }
}

#[cfg(target_os = "none")]
pub(crate) struct Sessions {
    clients: BTreeMap<u64, Session>,
    documents: BTreeMap<u64, DocSession>,
    frames: BTreeMap<u64, (u64, usize, u64)>,
    next_frame: usize,
}

#[cfg(target_os = "none")]
impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
            documents: BTreeMap::new(),
            frames: BTreeMap::new(),
            next_frame: 0,
        }
    }

    pub(crate) fn get(&mut self, sender: u64) -> &mut Session {
        self.clients.entry(sender).or_insert_with(Session::new)
    }

    pub(crate) fn doc(&mut self, sender: u64) -> &mut DocSession {
        self.documents.entry(sender).or_insert_with(DocSession::new)
    }

    pub(crate) fn attach_frame(&mut self, sender: u64, handle: u64, length: usize) -> bool {
        if length == 0 || length > hk_server_api::vfs::VFS_FRAME_LEN {
            return false;
        }
        let virt = VFS_SERVER_FRAME_BASE + self.next_frame as u64 * VFS_SERVER_FRAME_STRIDE;
        if frame_map(handle, virt) != length as u64 {
            return false;
        }
        if let Some((old_virt, old_len, old_handle)) =
            self.frames.insert(sender, (virt, length, handle))
        {
            let _ = frame_unmap(old_virt, old_len);
            let _ = close_handle(old_handle);
            unregister_client_frame(old_virt);
        }
        register_client_frame(virt, handle, length);
        self.next_frame += 1;
        true
    }

    pub(crate) fn detach(&mut self, sender: u64, filesystem: &mut Vfs) {
        if let Some((virt, length, handle)) = self.frames.remove(&sender) {
            let _ = frame_unmap(virt, length);
            let _ = close_handle(handle);
            unregister_client_frame(virt);
        }
        if let Some(session) = self.clients.remove(&sender) {
            for handle in session.handles.into_values() {
                let _ = filesystem.close(handle);
            }
        }
        self.documents.remove(&sender);
    }

    pub(crate) fn frame(&self, sender: u64) -> Option<(u64, usize)> {
        self.frames
            .get(&sender)
            .map(|(virt, length, _)| (*virt, *length))
    }
}
