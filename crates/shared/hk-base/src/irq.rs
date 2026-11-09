//! 设备中断向量约定。

/// 设备中断向量基址；IRQ `n` 使用 `IRQ_VECTOR_BASE + n`。
pub const IRQ_VECTOR_BASE: u8 = 0x40;

/// ACPI SCI 的虚拟 IRQ 编号（不占用 IOAPIC 线，只用于通知分发）。
pub const ACPI_SCI_IRQ: u8 = 32;

/// ACPI 电源按钮的虚拟 IRQ 编号。
pub const ACPI_POWER_IRQ: u8 = 33;

/// ACPI 睡眠按钮的虚拟 IRQ 编号。
pub const ACPI_SLEEP_IRQ: u8 = 34;
