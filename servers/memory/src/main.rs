//! Memory Server：按类型分配能力，并为匿名能力提供零页缺页。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, MEMORY_ALLOC, MEMORY_ALLOCATED, MEMORY_FRAME, MEMORY_FRAME_ALLOCATED,
    capability_bind_provider, capability_create, capability_create_lazy, capacity_alloc,
    capacity_free, capacity_info, exit, frame_alloc, frame_info, frame_pin, frame_release,
    frame_share, recv_data, reply, send_handle, write,
};
#[cfg(target_os = "none")]
use hk_syscall::{INITIAL_CAPACITY_HANDLE, PROVIDER_FAULT, PROVIDER_PROVIDE};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;
#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// 请求载荷：能力类型（1 字节）+ 标志（1 字节）+ 长度（8 字节小端）。
#[cfg(target_os = "none")]
const REQUEST_BYTES: usize = 1 + 1 + 8;
/// 请求标志：按需装入的匿名能力。
#[cfg(target_os = "none")]
const FLAG_LAZY: u8 = 1 << 0;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-memory: ready\n");
    if verify_capacity() {
        write(b"hk-server-memory: capacity ok\n");
    }
    loop {
        let mut request = [0_u8; REQUEST_BYTES];
        let (label, _word, sender, length) = recv_data(endpoint, &mut request);
        if label == MEMORY_FRAME {
            let size = (length >= 8)
                .then(|| u64::from_le_bytes(request[..8].try_into().unwrap_or([0; 8])))
                .unwrap_or(0) as usize;
            let Some(frame) = capacity_alloc(INITIAL_CAPACITY_HANDLE, size) else {
                let _ = send_handle(sender, MEMORY_FRAME_ALLOCATED, u64::MAX, 0);
                continue;
            };
            if send_handle(sender, MEMORY_FRAME_ALLOCATED, size as u64, frame) != 0 {
                exit(4);
            }
            continue;
        }
        if label == MEMORY_ALLOC {
            write(b"hk-server-memory: alloc\n");
            let kind = (length == REQUEST_BYTES)
                .then(|| CapabilityKind::from_u8(request[0]))
                .flatten();
            let lazy = request[1] & FLAG_LAZY != 0;
            let size = u64::from_le_bytes(request[2..].try_into().unwrap_or([0; 8])) as usize;
            let allocated = kind
                .zip((size > 0).then_some(size))
                .and_then(|(kind, size)| {
                    if lazy && kind == CapabilityKind::Anonymous {
                        let memory = capability_create_lazy(size)?;
                        let _ = capability_bind_provider(memory, endpoint);
                        Some(memory)
                    } else {
                        capability_create(kind, size)
                    }
                });
            let Some(memory) = allocated else {
                let _ = send_handle(sender, MEMORY_ALLOCATED, u64::MAX, 0);
                continue;
            };
            if send_handle(sender, MEMORY_ALLOCATED, size as u64, memory) != 0 {
                exit(3);
            }
            continue;
        }
        if label != PROVIDER_FAULT {
            let _ = reply(0, u64::MAX);
            continue;
        }
        write(b"hk-server-memory: fault\n");
        let Some(page) = frame_alloc(PAGE) else {
            let _ = reply(0, u64::MAX);
            continue;
        };
        if reply(PROVIDER_PROVIDE, page) != 0 {
            exit(2);
        }
    }
}

/// 验证 Root Server 授予的初始 Capacity 与 Frame 生命周期。
#[cfg(target_os = "none")]
fn verify_capacity() -> bool {
    let capacity = INITIAL_CAPACITY_HANDLE;
    let Some(before) = capacity_info(capacity) else {
        return false;
    };
    let Some(frame) = capacity_alloc(capacity, PAGE) else {
        return false;
    };
    let Some(info) = frame_info(frame) else {
        return false;
    };
    if info.len != PAGE as u64 || info.refs != 1 || info.pinned {
        return false;
    }
    if frame_share(frame) != Some(2)
        || frame_pin(frame, true) != 0
        || frame_pin(frame, false) != 0
        || frame_release(frame) != Some(1)
    {
        return false;
    }
    if capacity_free(capacity, frame) != 1 {
        return false;
    }
    capacity_info(capacity).is_some_and(|after| after.used == before.used)
}

hk_server_api::server_boilerplate!();
