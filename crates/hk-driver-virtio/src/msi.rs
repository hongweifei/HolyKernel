//! PCI MSI：单向量消息中断。
//!
//! 与 MSI-X 不同，MSI 直接在 capability 里写一组地址/数据，设备所有事件共用一个
//! 向量，不需要在 BAR 里维护表。

use hk_hal::irq::IRQ_VECTOR_BASE;

use crate::pci_config::Config;

/// MSI 目标：xAPIC 物理目的地模式，目标 APIC 为 0。
const MSI_ADDRESS: u32 = 0xFEE0_0000;
/// MSI capability 的 ID。
const MSI_CAPABILITY: u8 = 0x05;

/// message control 推导出的寄存器布局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    address64: bool,
    maskable: bool,
    /// 数据寄存器偏移。
    data: u8,
    /// 逐向量屏蔽寄存器偏移。
    mask: u8,
}

impl Layout {
    fn new(capability: u8, control: u16) -> Self {
        let address64 = control & 0x80 != 0;
        Self {
            address64,
            maskable: control & 0x100 != 0,
            data: capability + if address64 { 12 } else { 8 },
            mask: capability + if address64 { 16 } else { 12 },
        }
    }
}

/// 启用结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Msi {
    /// 设备中断使用的向量。
    pub vector: u8,
}

/// 为当前驱动实例的设备启用 MSI，向量为 `IRQ_VECTOR_BASE + irq`。
///
/// 返回 `None` 表示设备没有 MSI capability，调用方继续使用 INTx。
pub fn enable() -> Option<Msi> {
    let descriptor = crate::hal::grant_descriptor()?;
    let line = descriptor.irq?;
    let vector = IRQ_VECTOR_BASE + line;
    let config = Config::new(&descriptor);

    let capability = config.find(MSI_CAPABILITY)?;
    if capability & 3 != 0 {
        return None;
    }
    let control = config.read16(capability + 2);
    let layout = Layout::new(capability, control);

    config.write(capability + 4, MSI_ADDRESS);
    if layout.address64 {
        config.write(capability + 8, 0);
    }
    config.write16(layout.data, u16::from(vector));
    if layout.maskable {
        config.write(layout.mask, 0);
        config.write(layout.mask + 4, 0);
    }

    // 单条消息（multiple message enable = 0）并使能 MSI。
    config.write16(capability + 2, (control & !0x000E) | 0x0001);
    config.write(0x04, config.read(0x04) | 0x0400);
    Some(Msi { vector })
}

#[cfg(test)]
mod tests {
    use super::Layout;

    #[test]
    fn layout_32bit_without_masking() {
        let layout = Layout::new(0x40, 0x0001);
        assert!(!layout.address64);
        assert!(!layout.maskable);
        assert_eq!(layout.data, 0x48);
    }

    #[test]
    fn layout_64bit_with_masking() {
        let layout = Layout::new(0x50, 0x0180);
        assert!(layout.address64);
        assert!(layout.maskable);
        assert_eq!(layout.data, 0x5C);
        assert_eq!(layout.mask, 0x60);
    }
}
