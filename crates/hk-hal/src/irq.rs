//! 中断控制器与 IPI。

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::CpuId;

/// 设备中断向量基址；IRQ `n` 使用 `IRQ_VECTOR_BASE + n`。
pub const IRQ_VECTOR_BASE: u8 = 0x40;

/// 硬件中断号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Irq(u32);

impl Irq {
    /// 构造中断号。
    pub const fn new(number: u32) -> Self {
        Self(number)
    }

    /// 返回中断号。
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// 内核 Notification 的标识。
///
/// 中断上半部把信号送到该 Notification，由用户态驱动线程等待。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotificationId(u64);

impl NotificationId {
    /// 构造 Notification 标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 处理器间中断类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ipi {
    /// 重新调度。
    Reschedule,
    /// 刷新地址空间相关的 TLB 条目。
    TlbShootdown,
    /// 停机与重启流程使用。
    Halt,
}

/// 中断绑定错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqError {
    /// 该中断号已绑定到其他 Notification。
    AlreadyBound,
    /// 该中断号没有绑定。
    NotBound,
    /// 平台不支持该中断号。
    Unsupported,
}

impl fmt::Display for IrqError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyBound => formatter.write_str("中断号已绑定"),
            Self::NotBound => formatter.write_str("中断号未绑定"),
            Self::Unsupported => formatter.write_str("平台不支持该中断号"),
        }
    }
}

impl core::error::Error for IrqError {}

/// 中断控制器。
pub trait InterruptController: Send + Sync {
    /// 把中断号绑定到 Notification。
    fn bind(&self, irq: Irq, target: NotificationId) -> Result<(), IrqError>;

    /// 把中断绑定到通知，但只记录投递目标，不建立 IOAPIC 路由。
    ///
    /// MSI/MSI-X 由设备直接写 LAPIC，不需要也不应该占用一条 IOAPIC 线路。
    fn bind_direct(&self, irq: Irq, target: NotificationId) -> Result<(), IrqError> {
        self.bind(irq, target)
    }

    /// 解除中断号绑定。
    fn unbind(&self, irq: Irq) -> Result<(), IrqError>;

    /// 屏蔽中断号。
    fn mask(&self, irq: Irq);

    /// 解除屏蔽。
    fn unmask(&self, irq: Irq);

    /// 发送中断结束信号。
    fn eoi(&self, irq: Irq);

    /// 向目标 CPU 发送处理器间中断。
    fn send_ipi(&self, cpu: CpuId, ipi: Ipi);
}

/// 通用 IRQ 待处理位图。
pub struct IrqBitmap<const WORDS: usize> {
    bits: [AtomicU64; WORDS],
}

impl<const WORDS: usize> IrqBitmap<WORDS> {
    /// 构造空位图。
    pub const fn new() -> Self {
        Self {
            bits: [const { AtomicU64::new(0) }; WORDS],
        }
    }

    /// 记录一条待处理 IRQ。
    pub fn note(&self, line: usize) {
        let word = line / 64;
        if word < WORDS {
            self.bits[word].fetch_or(1_u64 << (line % 64), Ordering::Release);
        }
    }

    /// 取出并清除全部待处理 IRQ。
    pub fn take(&self) -> [u64; WORDS] {
        let mut pending = [0_u64; WORDS];
        for (slot, bits) in pending.iter_mut().zip(self.bits.iter()) {
            *slot = bits.swap(0, Ordering::AcqRel);
        }
        pending
    }

    /// 清空全部待处理 IRQ。
    pub fn clear(&self) {
        for bits in &self.bits {
            bits.store(0, Ordering::Release);
        }
    }
}

impl<const WORDS: usize> Default for IrqBitmap<WORDS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_round_trips_across_words() {
        let bitmap = IrqBitmap::<4>::new();
        bitmap.note(3);
        bitmap.note(70);
        assert_eq!(bitmap.take(), [1 << 3, 1 << 6, 0, 0]);
        assert_eq!(bitmap.take(), [0; 4]);
    }
}
