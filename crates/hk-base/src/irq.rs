//! 设备中断向量约定。

/// 设备中断向量基址；IRQ `n` 使用 `IRQ_VECTOR_BASE + n`。
pub const IRQ_VECTOR_BASE: u8 = 0x40;
