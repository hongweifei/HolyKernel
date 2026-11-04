//! Block Server：RAM disk 后端与固定块协议。
//!
//! 后续 virtio-blk 后端复用同一协议；当前先验证 BlockDevice → Server → Client
//! 数据路径。

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
mod virtio;

#[cfg(target_os = "none")]
use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};
#[cfg(target_os = "none")]
use hk_hal::DeviceGrant;
#[cfg(target_os = "none")]
use hk_server_api::block::{
    BLOCK_GRANT, BLOCK_REQUEST, BLOCK_RESPONSE, OP_CAPACITY, OP_FLUSH, OP_READ, OP_WRITE,
    STATUS_ERROR, STATUS_INVALID, STATUS_OK, STATUS_RANGE, decode_request, encode_response,
};
#[cfg(target_os = "none")]
use hk_server_api::{alloc, device_info, recv_data, recv_handle, send_data, write, write_u64};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 48 * 1024;
#[cfg(target_os = "none")]
const BLOCKS: u64 = 64;

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

#[cfg(target_os = "none")]
/// 服务入口。
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write("hk-server-block: start\n".as_bytes());
    let (label, _word, _sender, grant_handle) = recv_handle(endpoint);
    let mut grant = None;
    if label == BLOCK_GRANT && grant_handle != 0 {
        let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
        let length = device_info(grant_handle, &mut bytes) as usize;
        if length != u64::MAX as usize {
            grant = DeviceGrant::decode(&bytes[..length]);
        }
    }
    let virtio = grant.is_some();
    let mut disk: Box<dyn BlockDevice> = match grant.and_then(|descriptor| {
        virtio::VirtioBlock::new(grant_handle, descriptor)
            .map(|block| Box::new(block) as Box<dyn BlockDevice>)
    }) {
        Some(block) => block,
        None => {
            write("hk-server-block: ram disk ready\n".as_bytes());
            Box::new(MemoryDisk::new(BLOCKS, SECTOR_SIZE))
        }
    };

    if virtio {
        let mut sector = [0_u8; SECTOR_SIZE];
        let read = disk.read_blocks(0, &mut sector).is_ok();
        let pattern = [0x5A_u8; SECTOR_SIZE];
        let wrote = disk.write_blocks(1, &pattern).is_ok();
        let mut verify = [0_u8; SECTOR_SIZE];
        let verify_ok = disk.read_blocks(1, &mut verify).is_ok() && verify == pattern;
        let flush = disk.flush().is_ok();
        write(b"hk-server-block: virtio sectors=");
        write_u64(disk.capacity_blocks());
        write(b" read0=");
        write(if read { b"ok" } else { b"fail" });
        write(b" write1=");
        write(if wrote && verify_ok { b"ok" } else { b"fail" });
        write(b" flush=");
        write(if flush { b"ok" } else { b"fail" });
        write(b"\n");
    }

    let mut request = [0_u8; 4096];
    let mut body = [0_u8; 4096];
    let mut response = [0_u8; 4096];

    loop {
        let (label, _word, sender, length) = recv_data(endpoint, &mut request);
        if label != BLOCK_REQUEST {
            continue;
        }
        let Some(request) = decode_request(&request[..length.min(request.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let (status, written) = handle(disk.as_mut(), &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

#[cfg(target_os = "none")]
fn handle(
    disk: &mut dyn BlockDevice,
    request: &hk_server_api::block::Request<'_>,
    body: &mut [u8],
) -> (u8, usize) {
    match request.op {
        OP_READ => {
            let length = request.length as usize;
            if length == 0 || length > body.len() {
                return (STATUS_INVALID, 0);
            }
            match disk.read_blocks(request.block, &mut body[..length]) {
                Ok(length) => (STATUS_OK, length),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE => match disk.write_blocks(request.block, request.data) {
            Ok(length) => (STATUS_OK, length),
            Err(error) => (status(error), 0),
        },
        OP_FLUSH => match disk.flush() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_CAPACITY => {
            body[..8].copy_from_slice(&disk.capacity_bytes().to_le_bytes());
            (STATUS_OK, 8)
        }
        _ => (STATUS_INVALID, 0),
    }
}

#[cfg(target_os = "none")]
fn status(error: BlockError) -> u8 {
    match error {
        BlockError::OutOfRange => STATUS_RANGE,
        BlockError::InvalidLength => STATUS_INVALID,
        BlockError::ReadOnly | BlockError::Io => STATUS_ERROR,
    }
}

#[cfg(target_os = "none")]
fn reply(sender: u64, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, status, data) {
        let _ = send_data(sender, BLOCK_RESPONSE, &response[..length]);
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
    eprintln!("hk-server-block 只在 x86_64-unknown-none 目标上构建");
}
