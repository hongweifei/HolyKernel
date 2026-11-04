//! 设备授权：用户态驱动可获得的 MMIO、I/O 与 IRQ 资源描述。

use crate::pci::PciAddress;

/// MMIO 物理范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmioRange {
    /// 物理基址。
    pub base: u64,
    /// 长度。
    pub len: u64,
}

/// 端口 I/O 范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoRange {
    /// 端口基址。
    pub base: u16,
    /// 长度。
    pub len: u16,
}

/// 设备授权描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceGrant {
    /// 配置空间地址。
    pub address: PciAddress,
    /// 厂商编号。
    pub vendor: u16,
    /// 设备编号。
    pub device: u16,
    /// MMIO 范围。
    pub mmio: [Option<MmioRange>; 6],
    /// 端口 I/O 范围。
    pub io: [Option<IoRange>; 6],
    /// 中断线。
    pub irq: Option<u8>,
    /// capability ID。
    pub capabilities: [u8; 8],
    /// capability 数量。
    pub capability_count: usize,
}

impl DeviceGrant {
    /// 判断是否为 virtio 设备。
    pub fn is_virtio(&self) -> bool {
        self.capabilities[..self.capability_count.min(self.capabilities.len())].contains(&0x09)
    }
}
