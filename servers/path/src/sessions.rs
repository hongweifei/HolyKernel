//! Path 会话与客户端共享帧。

use alloc::collections::BTreeMap;

use hk_server_api::{
    RIGHT_READ, RIGHT_WRITE, capability_phys, close_handle, region_info, region_map, region_unmap,
};
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
        if length == 0 || length > hk_proto::vfs::VFS_BUFFER_LEN {
            let _ = close_handle(handle);
            return false;
        }
        if capability_phys(handle).is_none_or(|(_, capability_len)| capability_len < length as u64)
        {
            let _ = close_handle(handle);
            return false;
        }
        let Some(region) = region_map(handle, 0, length, RIGHT_READ | RIGHT_WRITE, 0) else {
            let _ = close_handle(handle);
            return false;
        };
        let Some((virt, _)) = region_info(region) else {
            let _ = region_unmap(region);
            let _ = close_handle(handle);
            return false;
        };
        if let Some((_old_virt, old_region, _old_len, old_handle)) =
            self.frames.insert(sender, (virt, region, length, handle))
        {
            let _ = region_unmap(old_region);
            let _ = close_handle(old_handle);
        }
        true
    }

    pub(crate) fn detach(&mut self, sender: u64, filesystem: &mut Vfs) {
        if let Some((_virt, region, _length, handle)) = self.frames.remove(&sender) {
            let _ = region_unmap(region);
            let _ = close_handle(handle);
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
