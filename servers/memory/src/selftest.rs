//! Memory 自检：初始 Capacity 与 Frame 生命周期。

use crate::PAGE;
use hk_server_api::{
    capacity_alloc, capacity_free, capacity_info, frame_info, frame_pin, frame_release,
    frame_share, ledger_enum,
};

/// 验证 Root Server 授予的初始 Capacity 与 Frame 生命周期。
#[cfg(target_os = "none")]
pub(crate) fn verify_capacity(capacity: u64) -> bool {
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

/// 验证容量 Ledger 枚举接口至少返回一条一致的记录。
#[cfg(target_os = "none")]
pub(crate) fn verify_ledger_enum() -> bool {
    ledger_enum(0).is_some_and(|info| info.capacity != 0 && info.used <= info.limit)
}
