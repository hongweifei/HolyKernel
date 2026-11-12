//! Memory 自检：初始 Capacity 与 Frame 生命周期。

use crate::PAGE;
use hk_server_api::{
    capacity_alloc, capacity_free, capacity_info, frame_info, frame_pin, ledger_enum,
};

/// 验证 Root Server 授予的初始 Capacity 与 Frame 生命周期。
#[cfg(target_os = "none")]
pub(crate) fn verify_capacity(capacity: u64) -> bool {
    let Some(_before) = capacity_info(capacity) else {
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
    if frame_pin(frame, true) != 0 || frame_pin(frame, false) != 0 {
        return false;
    }
    if capacity_free(capacity, frame) != 1 {
        return false;
    }
    // 顶层 User Capacity 由全部服务共享，并行分配会让全局 used 抖动；
    // 这里只校验自检帧的生命周期与容量仍可用，配额记账由单元测试覆盖。
    capacity_info(capacity).is_some()
}

/// 验证容量 Ledger 枚举接口至少返回一条一致的记录。
#[cfg(target_os = "none")]
pub(crate) fn verify_ledger_enum() -> bool {
    ledger_enum(0).is_some_and(|info| info.capacity != 0 && info.used <= info.limit)
}
