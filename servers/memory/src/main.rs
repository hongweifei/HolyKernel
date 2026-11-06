//! Memory Server：为缺页请求提供零页能力。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    CAPABILITY_ANONYMOUS, MEMORY_ALLOC, MEMORY_ALLOCATED, capability_bind_provider,
    capability_create, capability_create_lazy, exit, recv, reply_handle, send_handle, write,
};
#[cfg(target_os = "none")]
use hk_syscall::{PROVIDER_FAULT, PROVIDER_PROVIDE};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;
#[cfg(target_os = "none")]
const PAGE: usize = 4096;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-memory: ready\n");
    loop {
        let (label, _word, _sender) = recv(endpoint);
        if label == MEMORY_ALLOC {
            write(b"hk-server-memory: alloc\n");
            let length = _word as usize;
            let Some(memory) = capability_create_lazy(length) else {
                let _ = send_handle(_sender, MEMORY_ALLOCATED, u64::MAX, 0);
                continue;
            };
            let _ = capability_bind_provider(memory, endpoint);
            if send_handle(_sender, MEMORY_ALLOCATED, length as u64, memory) != 0 {
                exit(3);
            }
            continue;
        }
        if label != PROVIDER_FAULT {
            let _ = reply_handle(0, u64::MAX, 0);
            continue;
        }
        write(b"hk-server-memory: fault\n");
        let Some(page) = capability_create(CAPABILITY_ANONYMOUS, PAGE) else {
            let _ = reply_handle(0, u64::MAX, 0);
            continue;
        };
        if reply_handle(PROVIDER_PROVIDE, 0, page) != 0 {
            exit(2);
        }
    }
}

hk_server_api::server_boilerplate!();
