//! Provider 自动 swap 冒烟：制造压力，等待回收线程换出，再校验换入内容。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    MemoryClient, arena_grow, exit, memory_event_subscribe, memory_pressure, memory_pressure_full,
    self_notification, sleep_ticks, wait_notify, write,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 2 * 1024 * 1024;
/// 单页长度。
#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// 最多制造 16 MiB 压力。
#[cfg(target_os = "none")]
const MAX_PAGES: usize = 4096;
/// Memory Server 在 swap-test 模式设置的阈值。
#[cfg(target_os = "none")]
const PRESSURE_THRESHOLD: u64 = 90;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
fn fail(message: &[u8]) -> ! {
    write(b"hk-client-swap-test: fail ");
    write(message);
    write(b"\n");
    exit(1)
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, _capacity: u64) -> ! {
    let client = MemoryClient::new(endpoint);
    if !client.available() {
        fail(b"memory-service")
    }
    // 先扩展地址窗口，再让 FreeHeap 首次分配；否则 2 MiB 初始窗口放不下
    // 页表和测试数据。
    if arena_grow(32 * 1024 * 1024) == u64::MAX {
        fail(b"arena-grow")
    }
    let notification = self_notification();
    if memory_event_subscribe(notification) != 0 {
        fail(b"event-subscribe")
    }

    let mut pages: Vec<(u64, u64, u64, u8)> = Vec::new();
    let mut index = 0;
    while index < MAX_PAGES {
        let Some((capability, region, virt)) = client.map_lazy(PAGE) else {
            break;
        };
        let pattern = (index as u8).wrapping_mul(37).wrapping_add(11);
        // 先读触发读缺页，让 Provider 页进入能力页表；写缺页会被内核当作
        // COW 私有覆盖，不会写入共享能力页。
        // SAFETY: Region 已映射；读取触发 Provider 零页缺页。
        let _zero = unsafe { core::ptr::read_volatile(virt as *const u8) };
        // SAFETY: 上一条读取已装入该页。
        unsafe {
            core::ptr::write_volatile(virt as *mut u8, pattern);
        }
        pages.push((capability, region, virt, pattern));
        index += 1;
        if index % 32 == 0
            && let Some((free, total, _)) = memory_pressure()
            && total > 0
            && free.saturating_mul(100) / total <= PRESSURE_THRESHOLD
        {
            break;
        }
    }
    if pages.is_empty() {
        fail(b"allocate")
    }
    // 压力检查在 Memory Server 收到消息时执行；跨过阈值后追加一页，确保
    // 主循环还有一次机会通知回收线程。
    if index < MAX_PAGES
        && let Some((capability, region, virt)) = client.map_lazy(PAGE)
    {
        let pattern = (index as u8).wrapping_mul(37).wrapping_add(11);
        // SAFETY: Region 已映射；读取触发零页缺页，写不能先于读。
        let _zero = unsafe { core::ptr::read_volatile(virt as *const u8) };
        // SAFETY: 上一行读取已装入该页；写同一 Region 内已映射的 1 字节。
        unsafe {
            core::ptr::write_volatile(virt as *mut u8, pattern);
        }
        pages.push((capability, region, virt, pattern));
    }
    if let Some((free, total, threshold)) = memory_pressure() {
        hk_server_api::format::Line::new()
            .text(b"hk-client-swap-test: pressure ")
            .number(free.saturating_mul(100) / total.max(1))
            .text(b"% threshold ")
            .number(threshold)
            .text(b" pages ")
            .number(pages.len() as u64)
            .text(b"\n")
            .finish();
    }

    let mut swapped = 0;
    for _ in 0..1000 {
        if let Some(stats) = memory_pressure_full() {
            swapped = stats[7];
            if swapped > 0 {
                break;
            }
        }
        sleep_ticks(2);
    }
    if swapped == 0 {
        fail(b"no-reclaim")
    }
    // 换出事件已在 `finish_provider_reclaim` 中置位；等待确认通知链路。
    let swap_bit = 1_u64 << 1;
    if wait_notify(notification, swap_bit) & swap_bit == 0 {
        fail(b"event-notify")
    }

    let mut verified = 0;
    for (index, (_, _, virt, pattern)) in pages.iter().enumerate() {
        // SAFETY: 页可能已换出，读取触发 Provider 换入；内容必须与写入一致。
        let value = unsafe { core::ptr::read_volatile(*virt as *const u8) };
        if value != *pattern {
            hk_server_api::format::Line::new()
                .text(b"hk-client-swap-test: mismatch ")
                .number(index as u64)
                .text(b" expected ")
                .number(u64::from(*pattern))
                .text(b" actual ")
                .number(u64::from(value))
                .text(b"\n")
                .finish();
            fail(b"content")
        }
        verified += 1;
    }

    hk_server_api::format::Line::new()
        .text(b"hk-client-swap-test: pages ")
        .number(pages.len() as u64)
        .text(b" swapped ")
        .number(swapped)
        .text(b"\n")
        .finish();
    hk_server_api::format::Line::new()
        .text(b"hk-client-swap-test: verified ")
        .number(verified as u64)
        .text(b"\n")
        .finish();
    write(b"hk-client-swap-test: ok\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
