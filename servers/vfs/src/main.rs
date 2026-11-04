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
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_block::ipc::ClientDisk;
#[cfg(target_os = "none")]
use hk_fs_ext4::Ext4Backend;
#[cfg(target_os = "none")]
use hk_server_api::heap::BumpHeap;
#[cfg(target_os = "none")]
use hk_server_api::vfs::{
    OP_CLOSE, OP_LIST, OP_MKDIR, OP_MOUNT_TMPFS, OP_MOUNTS, OP_OPEN, OP_READ, OP_STAT, OP_TRUNCATE,
    OP_UMOUNT, OP_UNLINK, OP_WRITE, OPEN_CREATE, OPEN_READ, OPEN_WRITE, STATUS_ERROR,
    STATUS_EXISTS, STATUS_INVALID, STATUS_NOT_FOUND, STATUS_OK, STATUS_PERMISSION, VFS_REQUEST,
    VFS_RESPONSE, decode_request, encode_response,
};
#[cfg(target_os = "none")]
use hk_server_api::{recv_data, send_data, write, yield_now};
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
static ALLOCATOR: BumpHeap = BumpHeap::new(HEAP_SIZE);

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
}

#[cfg(target_os = "none")]
impl Sessions {
    const fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
        }
    }

    fn get(&mut self, sender: u64) -> &mut Session {
        self.clients.entry(sender).or_insert_with(Session::new)
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut filesystem = Vfs::new(Box::new(Tmpfs::new()));
    let _ = filesystem.create_dir("/tmp");
    let _ = filesystem.mount("/tmp", Box::new(Tmpfs::new()));
    let _ = filesystem.create_dir("/dev");
    let _ = filesystem.mount("/dev", Box::new(Devfs::new()));
    let _ = filesystem.create_dir("/proc");
    let _ = filesystem.mount("/proc", Box::new(Procfs::new()));
    let _ = filesystem.create_dir("/disk");
    if mount_ext4(&mut filesystem, endpoint) {
        write("hk-server-vfs: ext4 mounted\n".as_bytes());
        if ext4_self_test(&mut filesystem) {
            write("hk-server-vfs: ext4 读写 ok\n".as_bytes());
        } else {
            write("hk-server-vfs: ext4 读写失败\n".as_bytes());
        }
        let _ = filesystem.sync_all();
    } else {
        write("hk-server-vfs: ext4 不可用\n".as_bytes());
    }
    write("hk-server-vfs: tmpfs ready\n".as_bytes());

    let mut sessions = Sessions::new();
    let mut request = [0_u8; 4096];
    let mut body = [0_u8; 4093];
    let mut response = [0_u8; 4096];

    loop {
        let (label, _word, sender, length) = recv_data(endpoint, &mut request);
        if label != VFS_REQUEST {
            continue;
        }

        let Some(request) = decode_request(&request[..length.min(request.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let (status, written) = handle(&mut filesystem, sessions.get(sender), &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

/// 等待驱动就绪并在 `/disk` 挂载 ext4。
#[cfg(target_os = "none")]
fn mount_ext4(filesystem: &mut Vfs, endpoint: u64) -> bool {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
            let _ = yield_now();
            continue;
        };
        let Ok(backend) = Ext4Backend::mount(disk) else {
            return false;
        };
        return filesystem.mount("/disk", Box::new(backend)).is_ok();
    }
    false
}

/// 通过 VFS 路径写入并读回标记文件。
#[cfg(target_os = "none")]
fn ext4_self_test(filesystem: &mut Vfs) -> bool {
    const CONTENT: &[u8] = b"HolyKernel ext4 ok\n";
    let Ok(handle) = filesystem.open("/disk/hello", OpenFlags::CREATE) else {
        return false;
    };
    if filesystem.write(handle, CONTENT).is_err() {
        return false;
    }
    let _ = filesystem.close(handle);
    let Ok(handle) = filesystem.open("/disk/hello", OpenFlags::READ_ONLY) else {
        return false;
    };
    let mut buffer = [0_u8; 32];
    let read = filesystem.read(handle, &mut buffer).unwrap_or(0);
    let _ = filesystem.close(handle);
    read == CONTENT.len() && buffer[..read] == *CONTENT
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

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-vfs 只在 x86_64-unknown-none 目标上构建");
}
