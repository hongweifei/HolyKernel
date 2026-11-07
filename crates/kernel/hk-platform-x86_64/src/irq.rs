//! x86_64 设备中断向量与待处理位图。

use hk_hal::IrqBitmap;

/// 中断向量基址；与 `hk-hal` 共用同一常量。
pub const IRQ_VECTOR_BASE: u8 = hk_hal::irq::IRQ_VECTOR_BASE;

/// 支持的最大 IRQ 线数。
pub const IRQ_LINES: u8 = 32;

static PENDING: IrqBitmap<4> = IrqBitmap::new();

/// 记录一条待处理 IRQ。
pub fn note(line: u8) {
    PENDING.note(line as usize);
}

/// 取出并清除所有待处理 IRQ。
pub fn take_pending() -> [u64; 4] {
    PENDING.take()
}
