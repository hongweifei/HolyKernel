//! 设备中断待处理位图。
//!
//! 中断上半部只记录 IRQ 线，通知内核在调度循环中处理，避免在中断上下文
//! 获取运行时锁。

use core::sync::atomic::{AtomicU64, Ordering};

/// 中断向量基址；IRQ `n` 使用 `IRQ_VECTOR_BASE + n`。
pub const IRQ_VECTOR_BASE: u8 = 0x40;

/// 支持的最大 IRQ 线数。
pub const IRQ_LINES: u8 = 32;

static PENDING: AtomicU64 = AtomicU64::new(0);

/// 记录一条待处理 IRQ。
pub fn note(line: u8) {
    if line < IRQ_LINES {
        PENDING.fetch_or(1_u64 << line, Ordering::Release);
    }
}

/// 取出并清除所有待处理 IRQ。
pub fn take_pending() -> u64 {
    PENDING.swap(0, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_round_trip() {
        note(11);
        note(3);
        assert_eq!(take_pending(), (1 << 11) | (1 << 3));
        assert_eq!(take_pending(), 0);
    }
}
