//! VFS Server：tmpfs 根、请求协议与文件操作。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::collections::BTreeMap;
#[cfg(target_os = "none")]
use alloc::vec;

#[cfg(target_os = "none")]
use hk_block::ipc::{ClientDisk, register_client_frame, unregister_client_frame};
#[cfg(target_os = "none")]
use hk_fs_ext4::mount_at_with_wall_clock;
#[cfg(target_os = "none")]
use hk_fs_holyfs::HolyFs;
#[cfg(target_os = "none")]
use hk_hal::CivilTime;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::vfs::{
    OP_CLOSE, OP_LIST, OP_MKDIR, OP_MOUNT_TMPFS, OP_MOUNTS, OP_OPEN, OP_READ, OP_READ_FRAME,
    OP_STAT, OP_SYNC, OP_TRUNCATE, OP_UMOUNT, OP_UNLINK, OP_WRITE, OP_WRITE_FRAME, OPEN_CREATE,
    OPEN_READ, OPEN_WRITE, STATUS_ERROR, STATUS_EXISTS, STATUS_INVALID, STATUS_NOT_FOUND,
    STATUS_OK, STATUS_PERMISSION, VFS_ATTACH, VFS_DETACH, VFS_REQUEST, VFS_RESPONSE,
    VFS_SERVER_FRAME_BASE, VFS_SERVER_FRAME_STRIDE, decode_request, encode_response,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, frame_map, frame_unmap, recv_data_handle, send_data, write, yield_now,
};
#[cfg(target_os = "none")]
use hk_vfs::{Devfs, FileHandle, FileType, OpenFlags, Procfs, Tmpfs, Vfs, VfsError};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 768 * 1024;

/// 提供块设备的驱动服务。
#[cfg(target_os = "none")]
const BLOCK_SERVICE: &str = "hk-server-virtio";

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
struct Session {
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

    fn insert(&mut self, handle: FileHandle) -> u64 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.handles.insert(id, handle);
        id
    }

    fn get(&self, id: u64) -> Option<FileHandle> {
        self.handles.get(&id).copied()
    }

    fn remove(&mut self, id: u64) -> Option<FileHandle> {
        self.handles.remove(&id)
    }
}

#[cfg(target_os = "none")]
struct Sessions {
    clients: BTreeMap<u64, Session>,
    frames: BTreeMap<u64, (u64, usize, u64)>,
    next_frame: usize,
}

#[cfg(target_os = "none")]
impl Sessions {
    const fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
            frames: BTreeMap::new(),
            next_frame: 0,
        }
    }

    fn get(&mut self, sender: u64) -> &mut Session {
        self.clients.entry(sender).or_insert_with(Session::new)
    }

    fn attach_frame(&mut self, sender: u64, handle: u64, length: usize) -> bool {
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

    fn detach(&mut self, sender: u64, filesystem: &mut Vfs) {
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
    }

    fn frame(&self, sender: u64) -> Option<(u64, usize)> {
        self.frames
            .get(&sender)
            .map(|(virt, length, _)| (*virt, *length))
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut filesystem = Vfs::new(Box::new(Tmpfs::new()));
    let mut cmdline_buffer = [0_u8; 128];
    let cmdline = hk_server_api::boot_cmdline(&mut cmdline_buffer).unwrap_or("");
    let ext4_root = cmdline.contains("vfs.root=ext4");
    let holyfs = cmdline.contains("vfs.fs=holyfs");
    match hk_server_api::civil_time() {
        Some(time) if time.year >= 2000 => write("hk-server-vfs: rtc ok\n".as_bytes()),
        _ => write("hk-server-vfs: rtc 不可用\n".as_bytes()),
    }
    let target = if ext4_root && !holyfs { "/" } else { "/disk" };
    if target != "/" {
        let _ = filesystem.create_dir("/disk");
    }
    let mounted = if holyfs {
        mount_holyfs(&mut filesystem, endpoint, target)
    } else {
        mount_ext4(&mut filesystem, endpoint, target)
    };
    if mounted {
        if holyfs {
            write("hk-server-vfs: holyfs mounted\n".as_bytes());
        } else {
            write("hk-server-vfs: ext4 mounted\n".as_bytes());
        }
    } else {
        write("hk-server-vfs: 文件系统不可用\n".as_bytes());
    }
    let _ = filesystem.create_dir("/tmp");
    let _ = filesystem.mount("/tmp", Box::new(Tmpfs::new()));
    let _ = filesystem.create_dir("/dev");
    let _ = filesystem.mount("/dev", Box::new(Devfs::new()));
    let _ = filesystem.create_dir("/proc");
    let _ = filesystem.mount("/proc", Box::new(Procfs::new()));
    write("hk-server-vfs: tmpfs ready\n".as_bytes());

    let mut sessions = Sessions::new();
    let mut request = vec![0_u8; 64 * 1024];
    let mut body = vec![0_u8; 64 * 1024 - 4];
    let mut response = vec![0_u8; 64 * 1024];

    loop {
        let (label, word, sender, length, frame_handle) =
            recv_data_handle(endpoint, &mut request, 0);
        if frame_handle != 0 && label == VFS_ATTACH {
            let _ = sessions.attach_frame(sender, frame_handle, word as usize);
            continue;
        }
        if label == VFS_DETACH {
            sessions.detach(sender, &mut filesystem);
            continue;
        }
        if label != VFS_REQUEST {
            continue;
        }

        let Some(request) = decode_request(&request[..length.min(request.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let mapped = sessions.frame(sender).map(|(virt, length)| {
            // SAFETY: VFS_ATTACH 已把该客户端的共享帧映射到此地址。
            unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, length) }
        });
        let (status, written) = handle(
            &mut filesystem,
            sessions.get(sender),
            &request,
            &mut body,
            mapped,
        );
        reply(sender, status, &body[..written], &mut response);
    }
}

/// 平台民用时间来源。
#[cfg(target_os = "none")]
struct RealtimeClock;

#[cfg(target_os = "none")]
impl hk_fs_ext4::WallClock for RealtimeClock {
    fn now(&self) -> Option<CivilTime> {
        hk_server_api::civil_time()
    }
}

/// 等待驱动就绪并挂载 HolyFS。
#[cfg(target_os = "none")]
fn mount_holyfs(filesystem: &mut Vfs, endpoint: u64, target: &str) -> bool {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
            let _ = yield_now();
            continue;
        };
        let Ok(backend) = HolyFs::mount(disk) else {
            return false;
        };
        return filesystem.mount(target, Box::new(backend)).is_ok();
    }
    false
}

/// 等待驱动就绪并在 `/disk` 挂载 ext4。
#[cfg(target_os = "none")]
fn mount_ext4(filesystem: &mut Vfs, endpoint: u64, target: &str) -> bool {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
            let _ = yield_now();
            continue;
        };
        return mount_at_with_wall_clock(filesystem, target, disk, RealtimeClock).is_ok();
    }
    false
}

#[cfg(target_os = "none")]
fn frame_length(data: &[u8]) -> Option<usize> {
    Some(u64::from_le_bytes(data.get(..8)?.try_into().ok()?) as usize)
}

#[cfg(target_os = "none")]
fn reply(sender: u64, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, status, data) {
        let _ = send_data(sender, VFS_RESPONSE, &response[..length]);
    }
}

#[cfg(target_os = "none")]
fn handle(
    filesystem: &mut Vfs,
    session: &mut Session,
    request: &hk_server_api::vfs::Request<'_>,
    body: &mut [u8],
    frame: Option<&mut [u8]>,
) -> (u8, usize) {
    match request.op {
        OP_OPEN => {
            let mut flags = OpenFlags::default();
            flags.read = request.arg0 & OPEN_READ != 0;
            flags.write = request.arg0 & OPEN_WRITE != 0;
            flags.create = request.arg0 & OPEN_CREATE != 0;
            match filesystem.open(request.path, flags) {
                Ok(handle) => {
                    let id = session.insert(handle);
                    body[..8].copy_from_slice(&id.to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_CLOSE => {
            let Some(handle) = session.remove(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            match filesystem.close(handle) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_READ => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            match filesystem.read_at(handle, request.arg1, body) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            match filesystem.write_at(handle, request.arg1, request.data) {
                Ok(length) => {
                    body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_READ_FRAME => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            let Some(frame) = frame else {
                return (STATUS_INVALID, 0);
            };
            let Some(length) = frame_length(request.data) else {
                return (STATUS_INVALID, 0);
            };
            if length > frame.len() {
                return (STATUS_INVALID, 0);
            }
            match filesystem.read_at(handle, request.arg1, &mut frame[..length]) {
                Ok(length) => {
                    body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_FRAME => {
            let Some(handle) = session.get(request.arg0) else {
                return (STATUS_NOT_FOUND, 0);
            };
            let Some(frame) = frame else {
                return (STATUS_INVALID, 0);
            };
            let Some(length) = frame_length(request.data) else {
                return (STATUS_INVALID, 0);
            };
            if length > frame.len() {
                return (STATUS_INVALID, 0);
            }
            match filesystem.write_at(handle, request.arg1, &frame[..length]) {
                Ok(length) => {
                    body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_STAT => match filesystem.metadata(request.path) {
            Ok(metadata) => {
                body[0] = match metadata.kind {
                    FileType::File => 1,
                    FileType::Directory => 2,
                };
                body[1..9].copy_from_slice(&metadata.size.to_le_bytes());
                (STATUS_OK, 9)
            }
            Err(error) => (status(error), 0),
        },
        OP_TRUNCATE => match filesystem.truncate(request.path, request.arg0) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_UNLINK => match filesystem.unlink(request.path) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_LIST => match filesystem.readdir(request.path) {
            Ok(entries) => {
                let mut length = 0;
                for entry in entries {
                    let bytes = entry.name.as_bytes();
                    if length + bytes.len() + 1 > body.len() {
                        return (STATUS_ERROR, 0);
                    }
                    body[length..length + bytes.len()].copy_from_slice(bytes);
                    length += bytes.len();
                    body[length] = b'\n';
                    length += 1;
                }
                (STATUS_OK, length)
            }
            Err(error) => (status(error), 0),
        },
        OP_MKDIR => match filesystem.create_dir(request.path) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_MOUNT_TMPFS => {
            let _ = filesystem.create_dir(request.path);
            match filesystem.mount(request.path, Box::new(Tmpfs::new())) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_UMOUNT => match filesystem.umount(request.path) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_SYNC => match filesystem.sync_all() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_MOUNTS => {
            let mut length = 0;
            for mount in filesystem.mounts() {
                let bytes = mount.as_bytes();
                if length + bytes.len() + 1 > body.len() {
                    return (STATUS_ERROR, 0);
                }
                body[length..length + bytes.len()].copy_from_slice(bytes);
                length += bytes.len();
                body[length] = b'\n';
                length += 1;
            }
            (STATUS_OK, length)
        }
        _ => (STATUS_INVALID, 0),
    }
}
#[cfg(target_os = "none")]
fn status(error: VfsError) -> u8 {
    match error {
        VfsError::NotFound => STATUS_NOT_FOUND,
        VfsError::InvalidPath => STATUS_INVALID,
        VfsError::PermissionDenied => STATUS_PERMISSION,
        VfsError::AlreadyExists => STATUS_EXISTS,
        _ => STATUS_ERROR,
    }
}

hk_server_api::server_boilerplate!();
