//! Memory Server：按类型分配能力，并为匿名能力提供零页缺页。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec::Vec;
#[cfg(target_os = "none")]
use hk_syscall::{
    PROVIDER_FAULT, PROVIDER_FAULT_SWAPPED, PROVIDER_KIND_ANONYMOUS, PROVIDER_PROVIDE,
    PROVIDER_RECLAIM, PROVIDER_RECLAIM_BATCH, PROVIDER_RECLAIMED, PROVIDER_RELEASE,
};
#[cfg(target_os = "none")]
use hk_user_api::{
    CapabilityKind, MEMORY_ALLOC, MEMORY_ALLOCATED, MEMORY_FRAME, MEMORY_FRAME_ALLOCATED,
    RIGHT_READ, RIGHT_WRITE, boot_cmdline, capability_create, capability_swap, capacity_alloc,
    close_handle, frame_write, grant_capability, handle::Anonymous, handle::Endpoint,
    handle::Handle, handle_id, memory_pressure, message_words, notify, recv_data, reply,
    reply_handle, self_notification, send, send_handle, set_oom_policy, wait_notify, write,
};
#[cfg(target_os = "none")]
use spin::Mutex;

#[cfg(target_os = "none")]
mod selftest;
#[cfg(target_os = "none")]
mod swap;

hk_user_api::server_heap!(1024 * 1024);
#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// 请求载荷：能力类型（1 字节）+ 标志（1 字节）+ 长度（8 字节小端）。
#[cfg(target_os = "none")]
const REQUEST_BYTES: usize = 1 + 1 + 8;
/// 请求标志：按需装入的匿名能力。
#[cfg(target_os = "none")]
const FLAG_LAZY: u8 = 1 << 0;

/// Provider 保存的一页内容。
#[cfg(target_os = "none")]
enum Stored {
    /// 压缩块。
    Compressed(Vec<u8>),
    /// 不可压缩的原始页。
    Raw(Vec<u8>),
}

/// 压缩并保存一页；堆不足时返回 `None`。
#[cfg(target_os = "none")]
fn store_page(source: &[u8]) -> Option<Stored> {
    let raw = || {
        let mut raw = Vec::new();
        raw.try_reserve_exact(PAGE).ok()?;
        raw.extend_from_slice(source);
        Some(Stored::Raw(raw))
    };
    match swap::compress(source) {
        Some(compressed) => {
            let mut check = [0_u8; PAGE];
            if swap::decompress(&compressed, &mut check).is_some() {
                Some(Stored::Compressed(compressed))
            } else {
                raw()
            }
        }
        None => raw(),
    }
}

/// Provider 创建过的匿名能力：对象标识、长度与内核回收令牌。
#[cfg(target_os = "none")]
static TRACKED: Mutex<Vec<(u64, usize, u64)>> = Mutex::new(Vec::new());

#[cfg(target_os = "none")]
fn tracked_token(id: u64) -> Option<u64> {
    TRACKED
        .lock()
        .iter()
        .find(|(capability, _, _)| *capability == id)
        .map(|(_, _, token)| *token)
}

/// 压力回收线程：低于阈值时按对象标识重新取句柄并逐页请求换出。
///
/// 压缩/存储仍由主 Provider 循环完成；这里只决定“何时换出哪些页”。
#[cfg(target_os = "none")]
extern "C" fn reclaim_worker(notification: u64) -> ! {
    loop {
        let _ = wait_notify(notification, 1);
        if let Some((free, total, threshold)) = memory_pressure()
            && total > 0
            && free.saturating_mul(100) / total <= threshold
        {
            let tracked = TRACKED.lock().clone();
            'tracked: for (id, size, _) in tracked {
                if let Some((free, total, threshold)) = memory_pressure()
                    && total > 0
                    && free.saturating_mul(100) / total > threshold
                {
                    break 'tracked;
                }
                let Some(handle) = grant_capability(id, RIGHT_READ | RIGHT_WRITE) else {
                    continue;
                };
                let pages = size.div_ceil(PAGE);
                for page in 0..pages {
                    let _ = capability_swap(handle, (page * PAGE) as u64, PAGE);
                    if let Some((free, total, threshold)) = memory_pressure()
                        && total > 0
                        && free.saturating_mul(100) / total > threshold
                    {
                        break 'tracked;
                    }
                }
                let _ = close_handle(handle);
            }
        }
    }
}

/// 换出页存储：键为 `(能力对象, 页号)`，规模上限 256，线性扫描足够。
#[cfg(target_os = "none")]
type SwapStore = Vec<((u64, u64), Stored)>;

#[cfg(target_os = "none")]
fn swap_get(store: &SwapStore, key: (u64, u64)) -> Option<&Stored> {
    store
        .iter()
        .find(|(stored, _)| *stored == key)
        .map(|(_, value)| value)
}

#[cfg(target_os = "none")]
fn swap_contains(store: &SwapStore, key: (u64, u64)) -> bool {
    store.iter().any(|(stored, _)| *stored == key)
}

#[cfg(target_os = "none")]
fn swap_insert(store: &mut SwapStore, key: (u64, u64), value: Stored) {
    if let Some((_, slot)) = store.iter_mut().find(|(stored, _)| *stored == key) {
        *slot = value;
    } else {
        store.push((key, value));
    }
}

#[cfg(target_os = "none")]
fn swap_remove(store: &mut SwapStore, key: (u64, u64)) -> Option<Stored> {
    let position = store.iter().position(|(stored, _)| *stored == key)?;
    Some(store.swap_remove(position).1)
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
    let records = hk_user_api::numa_node_count().unwrap_or(0);
    hk_user_api::format::Line::new()
        .text(b"hk-server-memory: numa records ")
        .number(records as u64)
        .text(b"\n")
        .finish();
    let mut cmdline = [0_u8; 128];
    if boot_cmdline(&mut cmdline).is_some_and(|line| line.contains("swap-test")) {
        // 测试模式提高阈值、只回收不终止，便于在有限内存下触发自动换出。
        let result = set_oom_policy(90, 0);
        hk_user_api::format::Line::new()
            .text(b"hk-server-memory: swap policy ")
            .number(result)
            .text(b"\n")
            .finish();
    }
    let mut swapped: SwapStore = Vec::new();
    // 回收线程只发 swap 请求；主循环负责实际压缩与存储。
    let notification = self_notification();
    let _ = hk_thread::spawn_raw(reclaim_worker, notification, 64 * 1024);
    loop {
        // 批量回收最多携带 16 页；其它请求只使用前 10 字节。
        let mut request = alloc::vec![0_u8; PAGE * 16];

        let (label, _word, sender, length) = recv_data(endpoint, &mut request);

        if let Some((free, total, threshold)) = memory_pressure()
            && total > 0
            && free.saturating_mul(100) / total <= threshold
        {
            let _ = notify(notification, 1);
        }
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
                // 接收方配额不足时内核回滚转移，句柄仍归本服务；回收后
                // 用无句柄失败回包，不能因为一个客户端失败而终止服务。
                let _ = close_handle(frame);
                let _ = send(sender, MEMORY_FRAME_ALLOCATED, u64::MAX);
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
                        if memory.bind_provider(
                            Handle::<Endpoint>::from_raw(endpoint),
                            PROVIDER_KIND_ANONYMOUS,
                        ) != 0
                        {
                            let _ = close_handle(memory.raw());
                            return None;
                        }
                        if let Some(id) = handle_id(memory.raw()) {
                            TRACKED.lock().push((id, size, 0));
                        }
                        Some(memory.raw())
                    } else {
                        capability_create(kind, size)
                    }
                });
            let Some(memory) = allocated else {
                hk_user_api::format::Line::new()
                    .text(b"hk-server-memory: alloc failed ")
                    .number(size as u64)
                    .text(b" bytes\n")
                    .finish();
                let _ = send(sender, MEMORY_ALLOCATED, u64::MAX);
                continue;
            };
            if send_handle(sender, MEMORY_ALLOCATED, size as u64, memory) != 0 {
                let _ = close_handle(memory);
                let _ = send(sender, MEMORY_ALLOCATED, u64::MAX);
            }
            continue;
        }
        if label == PROVIDER_RELEASE {
            // 只有内核会发送能力析构通知；客户端伪造该消息不能删除任意
            // 能力的 swap 记录。
            if sender == 0
                && let Some(words) = message_words()
            {
                swapped.retain(|((capability, _), _)| *capability != words[0]);
                TRACKED
                    .lock()
                    .retain(|(capability, _, _)| *capability != words[0]);
            }
            continue;
        }
        if label == PROVIDER_RECLAIM_BATCH {
            let Some(words) = message_words() else {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            };
            let authorized =
                tracked_token(words[0]).is_some_and(|token| token != 0 && token == words[6]);
            if !authorized {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            let first = words[1] as usize;
            let count = words[4] as usize;
            if count == 0 || count > 16 || length != count * PAGE {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            let existing = (0..count)
                .filter(|index| swap_contains(&swapped, (words[0], (first + index) as u64)))
                .count();
            if swapped.len() + count - existing > 256 {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            let mut stored = Vec::with_capacity(count);
            let mut failed = false;
            for index in 0..count {
                let source = &request[index * PAGE..(index + 1) * PAGE];
                let Some(page) = store_page(source) else {
                    failed = true;
                    break;
                };
                stored.push((first + index, page));
            }
            if failed {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            for (page, content) in stored {
                swap_insert(&mut swapped, (words[0], page as u64), content);
            }
            let _ = reply(PROVIDER_RECLAIMED, 0);
            continue;
        }
        if label == PROVIDER_RECLAIM {
            let Some(words) = message_words() else {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            };
            let authorized =
                tracked_token(words[0]).is_some_and(|token| token != 0 && token == words[6]);
            if !authorized {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            if length != PAGE {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }
            let source = &request[..PAGE];
            if swapped.len() >= 256 && !swap_contains(&swapped, (words[0], words[1])) {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            }

            let Some(stored) = store_page(source) else {
                let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
                continue;
            };

            swap_insert(&mut swapped, (words[0], words[1]), stored);

            let _ = reply(PROVIDER_RECLAIMED, 0);

            continue;
        }
        if matches!(
            label,
            hk_syscall::PROVIDER_EVICT
                | hk_syscall::PROVIDER_WRITEBACK
                | hk_syscall::PROVIDER_ERROR
        ) {
            // 匿名页没有后备存储；这三类都是单向通知，不需要回包。
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
        // 能力 grow 后旧记录可能偏小；按实际缺页页号抬高回收线程的
        // 扫描上界。使用 try_lock：压力回收线程可能正在遍历，宁可这一次
        // 不更新也不能在 Provider 快速路径上阻塞。
        if let Some(mut tracked) = TRACKED.try_lock() {
            let required = (page_index as usize + 1).saturating_mul(PAGE);
            if let Some(entry) = tracked
                .iter_mut()
                .find(|(capability, _, _)| *capability == words[4])
            {
                entry.1 = entry.1.max(required);
                entry.2 = words[7];
            } else {
                tracked.push((words[4], required, words[7]));
            }
        }
        let mut buffer = [0_u8; PAGE];
        let stored = swap_get(&swapped, (words[4], page_index));
        if words[6] & u64::from(PROVIDER_FAULT_SWAPPED) != 0 && stored.is_none() {
            // 内核标记该页已换出，但本 Provider 实例没有对应内容：返回
            // 错误让缺页路径终止任务，避免把丢失的数据静默替换成零页。
            let _ = reply(hk_syscall::PROVIDER_ERROR, 0);
            continue;
        }
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
            let _ = hk_user_api::close_handle(frame);
            let _ = reply(0, u64::MAX);
            continue;
        }

        if reply_handle(PROVIDER_PROVIDE, 0, frame) != 0 {
            // 客户端可能已退出或句柄转移失败；关闭 Frame 后继续服务，
            // 不能因为单个缺页客户端让整个 Memory Server 退出。
            let _ = close_handle(frame);
            continue;
        }

        // 页已重新驻留；Provider 不再保留副本，下一次回收会重新写入。
        let _ = swap_remove(&mut swapped, (words[4], page_index));
    }
}

hk_user_api::server_boilerplate!();
