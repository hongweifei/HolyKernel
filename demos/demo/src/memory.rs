//! HMP 自检：经 Memory Server 申请匿名能力，缺页取零页写入后释放。

use hk_user_api::{
    MemoryClient, RIGHT_READ, RIGHT_WRITE, alloc, arena_info, capability_swap, close_handle,
    frame_info, frame_map, ledger_info, region_info, region_unmap,
};

/// 验证 Arena 分配与可写性。
pub(super) fn verify_arena() -> bool {
    let pointer = alloc(4096, 16);
    if pointer.is_null() {
        return false;
    }

    // SAFETY: 内核在任务 Arena 内分配了 4096 字节，地址有效且可写。
    let buffer = unsafe { core::slice::from_raw_parts_mut(pointer, 4096) };
    buffer.fill(0xA5);
    buffer.iter().all(|&byte| byte == 0xA5)
}

/// 验证匿名能力、换出换入、Frame 映射与容量归因。
///
/// 默认服务使用懒 TaskImage：首次执行某段代码会经 Provider 装入代码页并计入
/// 任务 Ledger。先完整跑一遍预热，再在第二遍比较水位，避免把代码页缺页误判
/// 成 HMP 泄漏。
pub(super) fn verify_memory(endpoint: u64) -> bool {
    if !exercise_memory(endpoint) {
        return false;
    }
    let Some(before) = ledger_info() else {
        return false;
    };
    if !exercise_memory(endpoint) {
        return false;
    }
    let Some(after) = ledger_info() else {
        return false;
    };
    after.charged == before.charged
}

/// 执行一次匿名能力、换出换入与 Frame 映射校验。
fn exercise_memory(endpoint: u64) -> bool {
    const PAGE: usize = 4096;
    let client = MemoryClient::new(endpoint);
    if !client.available() {
        return false;
    }
    let Some((capability, region, virt)) = client.map_lazy(PAGE) else {
        return false;
    };
    let Some(arena) = arena_info() else {
        return false;
    };
    if arena.size == 0 || arena.regions == 0 || arena.used < PAGE as u64 {
        return false;
    }
    // SAFETY: 区域已按一页映射进本任务；首次读取触发 Provider 缺页。
    let zero = unsafe { core::ptr::read_volatile(virt as *const u8) };
    // SAFETY: 上一条读取已装入该页，写入的是本任务私有页。
    unsafe { core::ptr::write_volatile(virt as *mut u8, 0xAB) };
    // SAFETY: 读回刚写入的字节校验映射与内容。
    let written = unsafe { core::ptr::read_volatile(virt as *const u8) };
    // Provider 回收这一页；随后再次读取必须换入同一内容。
    let swapped = capability_swap(capability, 0, PAGE) == PAGE as u64;
    // SAFETY: 页已换出，读取触发 Provider 换入。
    let restored = unsafe { core::ptr::read_volatile(virt as *const u8) };
    let _ = region_unmap(region);
    let _ = close_handle(capability);
    let Some(frame) = client.frame(PAGE) else {
        return false;
    };
    let Some(frame_info) = frame_info(frame) else {
        return false;
    };
    if frame_info.len != PAGE as u64 || frame_info.refs != 1 {
        return false;
    }
    let Some(frame_region) = frame_map(frame, RIGHT_READ | RIGHT_WRITE, 0) else {
        return false;
    };
    let Some((frame_virt, _)) = region_info(frame_region) else {
        return false;
    };
    // SAFETY: Frame 已映射进本任务，按一页读写校验。
    unsafe {
        core::ptr::write_volatile(frame_virt as *mut u8, 0xCD);
        if core::ptr::read_volatile(frame_virt as *const u8) != 0xCD {
            return false;
        }
    }
    let _ = region_unmap(frame_region);
    zero == 0 && written == 0xAB && swapped && restored == 0xAB
}
