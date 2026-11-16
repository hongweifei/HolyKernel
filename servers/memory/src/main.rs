//! Memory Server：按类型分配能力，并为匿名能力提供零页缺页。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::collections::BTreeMap;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, MEMORY_ALLOC, MEMORY_ALLOCATED, MEMORY_FRAME, MEMORY_FRAME_ALLOCATED,
    capability_create, capacity_alloc, exit, frame_write, handle::Anonymous, handle::Endpoint,
    handle::Handle, message_words, recv_data, reply, reply_handle, send, send_handle, write,
};
#[cfg(target_os = "none")]
use hk_syscall::{
    PROVIDER_FAULT, PROVIDER_KIND_ANONYMOUS, PROVIDER_PROVIDE, PROVIDER_RECLAIM,
    PROVIDER_RECLAIMED, PROVIDER_RELEASE,
};

#[cfg(target_os = "none")]
mod selftest;
#[cfg(target_os = "none")]
mod swap;

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;
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

/// Provider 保存的一页内容。
#[cfg(target_os = "none")]
enum Stored {
    /// 压缩块。
    Compressed(Vec<u8>),
    /// 不可压缩的原始页。
    Raw(Vec<u8>),
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, capacity: u64) -> ! {
    write(b"hk-server-memory: ready\n");
    if selftest::verify_capacity(capacity) {
        write(b"hk-server-memory: capacity ok\n");
    }
    if selftest::verify_ledger_enum() {
        write(b"hk-server-memory: ledger ok\n");
    }
    let records = hk_server_api::numa_node_count().unwrap_or(0);
    hk_server_api::format::Line::new()
        .text(b"hk-server-memory: numa records ")
        .number(records as u64)
        .text(b"\n")
        .finish();
    let mut swapped: BTreeMap<(u64, u64), Stored> = BTreeMap::new();
    loop {
        // 回收请求可能携带整页内容；其它请求只使用前 10 字节。
        let mut request = [0_u8; PAGE];
        let (label, _word, sender, length) = recv_data(endpoint, &mut request);
        if label == MEMORY_FRAME {
            let size = if length >= 8 {
                u64::from_le_bytes(request[..8].try_into().unwrap_or([0; 8])) as usize
            } else {
                0
            };
            let Some(frame) = capacity_alloc(capacity, size) else {
                // 失败回复不带句柄；用 send_handle(…, 0) 会被内核当作坏句柄丢弃，
                // 请求方会永久阻塞在接收上。
                let _ = send(sender, MEMORY_FRAME_ALLOCATED, u64::MAX);
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
            let size =
                u64::from_le_bytes(request[2..REQUEST_BYTES].try_into().unwrap_or([0; 8])) as usize;
            let allocated = kind
                .zip((size > 0).then_some(size))
                .and_then(|(kind, size)| {
                    if lazy && kind == CapabilityKind::Anonymous {
                        let memory = Handle::<Anonymous>::create_lazy(size)?;
                        let _ = memory.bind_provider(
                            Handle::<Endpoint>::from_raw(endpoint),
                            PROVIDER_KIND_ANONYMOUS,
                        );
                        Some(memory.raw())
                    } else {
                        capability_create(kind, size)
                    }
                });
            let Some(memory) = allocated else {
                hk_server_api::format::Line::new()
                    .text(b"hk-server-memory: alloc failed ")
                    .number(size as u64)
                    .text(b" bytes\n")
                    .finish();
                let _ = send(sender, MEMORY_ALLOCATED, u64::MAX);
                continue;
            };
            if send_handle(sender, MEMORY_ALLOCATED, size as u64, memory) != 0 {
                exit(3);
            }
            continue;
        }
        if label == PROVIDER_RELEASE {
            if let Some(words) = message_words() {
                swapped.retain(|(capability, _), _| *capability != words[0]);
            }
            continue;
        }
        if label == PROVIDER_RECLAIM {
            let Some(words) = message_words() else {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            };
            if length != PAGE {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            let source = &request[..PAGE];
            if swapped.len() >= 256 && !swapped.contains_key(&(words[0], words[1])) {
                // 服务堆有界：满了就让内核保留页面，而不是 panic。
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            let raw = || {
                let mut raw = Vec::new();
                raw.try_reserve_exact(PAGE).ok()?;
                raw.extend_from_slice(source);
                Some(Stored::Raw(raw))
            };
            let stored = match swap::compress(source) {
                Some(compressed) => {
                    let mut check = [0_u8; PAGE];
                    if swap::decompress(&compressed, &mut check).is_some() {
                        Stored::Compressed(compressed)
                    } else if let Some(raw) = raw() {
                        raw
                    } else {
                        let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                        continue;
                    }
                }
                None => {
                    let Some(raw) = raw() else {
                        let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                        continue;
                    };
                    raw
                }
            };
            swapped.insert((words[0], words[1]), stored);
            let _ = reply(PROVIDER_RECLAIMED, 0);
            continue;
        }
        if matches!(
            label,
            hk_syscall::PROVIDER_EVICT
                | hk_syscall::PROVIDER_WRITEBACK
                | hk_syscall::PROVIDER_ERROR
        ) {
            // 匿名页没有后备存储；驱逐、回写与错误通知只需确认。
            let _ = reply(0, 0);
            continue;
        }
        if label != PROVIDER_FAULT {
            let _ = reply(0, u64::MAX);
            continue;
        }
        write(b"hk-server-memory: fault\n");
        let Some(words) = message_words() else {
            let _ = reply(0, u64::MAX);
            continue;
        };
        let page_index = words[0] / PAGE as u64;
        let mut buffer = [0_u8; PAGE];
        let stored = swapped.get(&(words[4], page_index));
        if let Some(stored) = stored {
            match stored {
                Stored::Compressed(compressed) => {
                    if swap::decompress(compressed, &mut buffer).is_none() {
                        let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                        continue;
                    }
                }
                Stored::Raw(raw) => {
                    if raw.len() != PAGE {
                        let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                        continue;
                    }
                    buffer.copy_from_slice(raw);
                }
            }
        }
        let Some(frame) = capacity_alloc(capacity, PAGE) else {
            let _ = reply(0, u64::MAX);
            continue;
        };
        if frame_write(frame, 0, &buffer) != 0 {
            let _ = hk_server_api::close_handle(frame);
            let _ = reply(0, u64::MAX);
            continue;
        }
        if reply_handle(PROVIDER_PROVIDE, 0, frame) != 0 {
            exit(2);
        }
        // 页已重新驻留；Provider 不再保留副本，下一次回收会重新写入。
        swapped.remove(&(words[4], page_index));
    }
}

hk_server_api::server_boilerplate!();
