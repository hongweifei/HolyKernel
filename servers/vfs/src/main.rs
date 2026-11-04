//! VFS Server：tmpfs 根、请求协议与文件操作。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use core::alloc::{GlobalAlloc, Layout};
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "none")]
use hk_server_api::vfs::{
    OP_CLOSE, OP_LIST, OP_MKDIR, OP_MOUNT_TMPFS, OP_MOUNTS, OP_OPEN, OP_READ, OP_STAT, OP_TRUNCATE,
    OP_UMOUNT, OP_UNLINK, OP_WRITE, OPEN_CREATE, OPEN_READ, OPEN_WRITE, STATUS_ERROR,
    STATUS_EXISTS, STATUS_INVALID, STATUS_NOT_FOUND, STATUS_OK, STATUS_PERMISSION, VFS_REQUEST,
    VFS_RESPONSE, decode_request, encode_response,
};
#[cfg(target_os = "none")]
use hk_server_api::{alloc, recv_data, send_data, write};
#[cfg(target_os = "none")]
use hk_vfs::{Devfs, FileHandle, FileType, OpenFlags, Procfs, Tmpfs, Vfs, VfsError};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 48 * 1024;

#[cfg(target_os = "none")]
struct BumpHeap;

#[cfg(target_os = "none")]
static HEAP_BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "none")]
static HEAP_NEXT: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "none")]
unsafe impl GlobalAlloc for BumpHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut base = HEAP_BASE.load(Ordering::Acquire);
        if base == 0 {
            base = alloc(HEAP_SIZE, 16) as usize;
            if base == 0 {
                return core::ptr::null_mut();
            }
            HEAP_BASE.store(base, Ordering::Release);
            HEAP_NEXT.store(base, Ordering::Release);
        }

        let mut current = HEAP_NEXT.load(Ordering::Acquire);
        loop {
            let aligned = (current + layout.align() - 1) & !(layout.align() - 1);
            let Some(end) = aligned.checked_add(layout.size()) else {
                return core::ptr::null_mut();
            };
            if end > base + HEAP_SIZE {
                return core::ptr::null_mut();
            }
            match HEAP_NEXT.compare_exchange(current, end, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return aligned as *mut u8,
                Err(actual) => current = actual,
            }
        }
    }

    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: Layout) {}
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: BumpHeap = BumpHeap;

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
    write("hk-server-vfs: tmpfs ready\n".as_bytes());

    let mut request = [0_u8; 4096];
    let mut body = [0_u8; 4096];
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
        let (status, written) = handle(&mut filesystem, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
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
                    body[..8].copy_from_slice(&handle.id().to_le_bytes());
                    (STATUS_OK, 8)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_CLOSE => match filesystem.close(FileHandle::from_id(request.arg0)) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_READ => {
            match filesystem.read_at(FileHandle::from_id(request.arg0), request.arg1, body) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE => match filesystem.write_at(
            FileHandle::from_id(request.arg0),
            request.arg1,
            request.data,
        ) {
            Ok(length) => {
                body[..8].copy_from_slice(&(length as u64).to_le_bytes());
                (STATUS_OK, 8)
            }
            Err(error) => (status(error), 0),
        },
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
