//! Path 会话与客户端共享帧。

use alloc::collections::BTreeMap;

use hk_block::ipc::{register_client_mapping, unregister_client_mapping};
use hk_server_api::{close_handle, map_attach, map_unmap};
use hk_vfs::{FileHandle, Vfs};

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
    frames: BTreeMap<u64, (u64, usize, u64)>,
}

#[cfg(target_os = "none")]
impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
            frames: BTreeMap::new(),
        }
    }

    pub(crate) fn get(&mut self, sender: u64) -> &mut Session {
        self.clients.entry(sender).or_insert_with(Session::new)
    }

    pub(crate) fn attach_frame(&mut self, sender: u64, handle: u64, length: usize) -> bool {
        if length == 0 || length > hk_server_api::vfs::VFS_FRAME_LEN {
            return false;
        }
        let Some((mapped, virt)) = map_attach(handle) else {
            return false;
        };
        if mapped != length as u64 {
            let _ = map_unmap(virt, mapped as usize);
            return false;
        }
        if let Some((old_virt, old_len, old_handle)) =
            self.frames.insert(sender, (virt, length, handle))
        {
            let _ = map_unmap(old_virt, old_len);
            let _ = close_handle(old_handle);
            unregister_client_mapping(old_virt);
        }
        register_client_mapping(virt, handle, length);
        true
    }

    pub(crate) fn detach(&mut self, sender: u64, filesystem: &mut Vfs) {
        if let Some((virt, length, handle)) = self.frames.remove(&sender) {
            let _ = map_unmap(virt, length);
            let _ = close_handle(handle);
            unregister_client_mapping(virt);
        }
        if let Some(session) = self.clients.remove(&sender) {
            for handle in session.handles.into_values() {
                let _ = filesystem.close(handle);
            }
        }
    }

    pub(crate) fn frame(&self, sender: u64) -> Option<(u64, usize)> {
        self.frames
            .get(&sender)
            .map(|(virt, length, _)| (*virt, *length))
    }
}
