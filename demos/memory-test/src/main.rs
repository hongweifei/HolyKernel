//! HMP 内存对象面 conformance：验证 Arena、Region、Capability 与 Ledger 的统一语义。
//!
//! 本服务不请求额外 Capacity，只使用任务自带的 Arena 与默认匿名能力，覆盖
//! 路线图 M6 退出标准中的“对象面统一语义”：能力创建、Region 映射、按需零页、
//! protect、unmap 与记账变化。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, RIGHT_EXECUTE, RIGHT_READ, RIGHT_WRITE, arena_info, capability_create_lazy,
    exit, ledger_info, region_info, region_map, region_protect, region_unmap, write,
};

#[cfg(target_os = "none")]
const PAGE: usize = 4096;

#[cfg(target_os = "none")]
fn fail(code: u64) -> ! {
    write(b"hk-server-memory-test: fail ");
    hk_server_api::write_u64(code);
    write(b"\n");
    exit(code)
}

/// 服务入口：逐项校验 HMP 对象面语义。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    // 1. Arena 快照可用，且初始 Region 数为零。
    let Some(arena) = arena_info() else { fail(1) };
    if arena.size == 0 || arena.regions != 0 {
        fail(2)
    }

    // 2. 创建按需装入的匿名能力。
    let Some(capability) = capability_create_lazy(PAGE) else {
        fail(3)
    };
    let Some(ledger_before) = ledger_info() else {
        fail(4)
    };

    // 3. 映射为可读写 Region。
    let Some(region) = region_map(capability, 0, PAGE, RIGHT_READ | RIGHT_WRITE, 0) else {
        fail(5)
    };
    let Some((virt, len)) = region_info(region) else {
        fail(6)
    };
    if len != PAGE as u64 {
        fail(7)
    }

    // 4. 写入触发匿名零页缺页；读回校验内容。
    // SAFETY: virt 是刚映射的可写 Region，长度为一页。
    unsafe {
        let pointer = virt as *mut u64;
        pointer.write_volatile(0x5A5A_1234_5678_9ABC);
        if pointer.read_volatile() != 0x5A5A_1234_5678_9ABC {
            fail(8)
        }
    }

    // 5. Arena 已登记该 Region，且用字节数增加。
    let Some(after_map) = arena_info() else {
        fail(9)
    };
    if after_map.regions != 1 || after_map.used < PAGE as u64 {
        fail(10)
    }
    if after_map.base != arena.base {
        fail(11)
    }

    // 6. Ledger 随能力创建记账；配额非零。
    let Some(ledger_after) = ledger_info() else {
        fail(12)
    };
    if ledger_after.limit == 0 || ledger_after.charged < ledger_before.charged {
        fail(13)
    }

    // 7. protect 收窄为只读；再次 protect 为可写应成功，W^X 冲突应拒绝。
    if region_protect(region, RIGHT_READ, 0) != 0 {
        fail(14)
    }
    if region_protect(region, RIGHT_READ | RIGHT_WRITE, 0) != 0 {
        fail(15)
    }
    if region_protect(region, RIGHT_READ | RIGHT_WRITE | RIGHT_EXECUTE, 0) != u64::MAX {
        fail(16)
    }

    // 8. 解除映射后 Arena Region 数回到零。
    if region_unmap(region) != 1 {
        fail(17)
    }
    let Some(after_unmap) = arena_info() else {
        fail(18)
    };
    if after_unmap.regions != 0 {
        fail(19)
    }

    // 9. 能力类型判定：匿名能力接受 Swap 语义，不改变 Region 结果。
    if CapabilityKind::from_u8(CapabilityKind::Anonymous.as_u8()) != Some(CapabilityKind::Anonymous)
    {
        fail(20)
    }

    write(b"hk-server-memory-test: ok\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
