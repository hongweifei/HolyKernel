//! Path 会话与客户端共享帧。

use alloc::collections::BTreeMap;

use hk_block::ipc::{register_client_mapping, unregister_client_mapping};
use hk_server_api::{capability_phys, close_handle, map_attach, region_unmap};
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
    frames: BTreeMap<u64, (u64, u64, usize, u64)>,
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

    pub(crate) fn attach_mapping(&mut self, sender: u64, handle: u64, length: usize) -> bool {
        if length == 0 || length > hk_server_api::vfs::VFS_BUFFER_LEN {
            return false;
        }
        let Some((region, virt)) = map_attach(handle) else {
            return false;
        };
        if capability_phys(handle).is_none_or(|(_, cap_len)| cap_len != length as u64) {
            let _ = region_unmap(region);
            return false;
        }
        if let Some((old_virt, old_region, _old_len, old_handle)) =
            self.frames.insert(sender, (virt, region, length, handle))
        {
            let _ = region_unmap(old_region);
            let _ = close_handle(old_handle);
            unregister_client_mapping(old_virt);
        }
        register_client_mapping(virt, handle, length);
        true
    }

    pub(crate) fn detach(&mut self, sender: u64, filesystem: &mut Vfs) {
        if let Some((virt, region, _length, handle)) = self.frames.remove(&sender) {
            let _ = region_unmap(region);
            let _ = close_handle(handle);
            unregister_client_mapping(virt);
        }
        if let Some(session) = self.clients.remove(&sender) {
            for handle in session.handles.into_values() {
                let _ = filesystem.close(handle);
            }
        }
    }

    pub(crate) fn mapping(&self, sender: u64) -> Option<(u64, usize)> {
        self.frames
            .get(&sender)
            .map(|(virt, _, length, _)| (*virt, *length))
    }
}
