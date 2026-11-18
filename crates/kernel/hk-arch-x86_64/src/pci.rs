//! PCI 配置空间访问：优先 MCFG ECAM，缺失时回退 legacy 机制 #1。
//!
//! Legacy 通过 `0xCF8` 写入配置地址、`0xCFC` 读写数据，只支持 32 位对齐访问；
//! ECAM 使用 MCFG 描述的 MMIO 窗口，按 bus/device/function/offset 直接寻址。

use hk_device::{PciAddress, PciConfig};
use hk_mmio::Mmio;
use spin::Mutex;

use crate::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;
const ENABLE: u32 = 1 << 31;

/// ECAM 窗口（虚拟地址）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ecam {
    /// 映射后的虚拟基址。
    pub base: u64,
    /// PCI 段号。
    pub segment: u16,
    /// 起始总线。
    pub bus_start: u8,
    /// 结束总线（含）。
    pub bus_end: u8,
}

impl Ecam {
    fn offset(&self, address: PciAddress) -> Option<usize> {
        if address.segment != self.segment
            || address.bus < self.bus_start
            || address.bus > self.bus_end
            || address.device >= 32
            || address.function >= 8
        {
            return None;
        }
        let offset = (usize::from(address.bus - self.bus_start) << 20)
            | (usize::from(address.device) << 15)
            | (usize::from(address.function) << 12)
            | usize::from(address.offset & 0xFC);
        Some(offset)
    }
}

static ECAM: Mutex<alloc::vec::Vec<Ecam>> = Mutex::new(alloc::vec::Vec::new());

/// 登记 MCFG 映射出的 ECAM 窗口。
pub fn init_ecam(ecam: Ecam) {
    let mut windows = ECAM.lock();
    if !windows.contains(&ecam) {
        windows.push(ecam);
    }
}

/// 返回第一个 ECAM 窗口。
pub fn ecam() -> Option<Ecam> {
    ECAM.lock().first().copied()
}

/// x86 PCI 配置访问；ECAM 可用时优先使用。
pub struct X86Pci;

impl X86Pci {
    fn legacy_address(address: PciAddress) -> u32 {
        ENABLE
            | (u32::from(address.bus) << 16)
            | (u32::from(address.device & 0x1F) << 11)
            | (u32::from(address.function & 0x07) << 8)
            | u32::from(address.offset & 0xFC)
    }
}

/// 全局 PCI 配置访问。
pub const PCI: X86Pci = X86Pci;

impl PciConfig for X86Pci {
    fn read32(&self, address: PciAddress) -> u32 {
        if let Some(ecam) = ECAM
            .lock()
            .iter()
            .find(|ecam| ecam.offset(address).is_some())
            .copied()
            && let Some(offset) = ecam.offset(address)
        {
            // SAFETY: ECAM 窗口已映射为设备页，偏移落在对应总线窗口内。
            return unsafe { Mmio::new(ecam.base as usize).read_u32(offset) };
        }
        if address.segment != 0 {
            return u32::MAX;
        }
        // SAFETY: PCI 配置端口在 x86_64 平台上固定存在；写入地址后读取数据端口。
        unsafe {
            outl(CONFIG_ADDRESS, Self::legacy_address(address));
            inl(CONFIG_DATA)
        }
    }

    fn write32(&self, address: PciAddress, value: u32) {
        if let Some(ecam) = ECAM
            .lock()
            .iter()
            .find(|ecam| ecam.offset(address).is_some())
            .copied()
            && let Some(offset) = ecam.offset(address)
        {
            // SAFETY: ECAM 窗口已映射为设备页，偏移落在对应总线窗口内。
            unsafe { Mmio::new(ecam.base as usize).write_u32(offset, value) };
            return;
        }
        if address.segment != 0 {
            return;
        }
        // SAFETY: 同上；只写配置数据端口。
        unsafe {
            outl(CONFIG_ADDRESS, Self::legacy_address(address));
            outl(CONFIG_DATA, value);
        }
    }
}

/// 读取配置空间中的一个 32 位字。
pub fn config_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    PCI.read32(PciAddress::new(bus, device, function, offset))
}

/// 写入配置空间中的一个 32 位字。
pub fn config_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    PCI.write32(PciAddress::new(bus, device, function, offset), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecam_offsets_follow_pci_layout() {
        let ecam = Ecam {
            base: 0xFFFF_C000_0000_0000,
            segment: 0,
            bus_start: 0,
            bus_end: 0xFF,
        };
        assert_eq!(
            ecam.offset(PciAddress::new(1, 2, 3, 0x10)),
            Some((1 << 20) | (2 << 15) | (3 << 12) | 0x10)
        );
        assert_eq!(
            ecam.offset(PciAddress::new(1, 2, 3, 0x13)),
            Some((1 << 20) | (2 << 15) | (3 << 12) | 0x10)
        );
        assert!(ecam.offset(PciAddress::new(0, 0, 0, 0)).is_some());
    }

    #[test]
    fn ecam_rejects_other_segments_and_buses() {
        let ecam = Ecam {
            base: 0,
            segment: 1,
            bus_start: 4,
            bus_end: 8,
        };
        let valid = PciAddress {
            segment: 1,
            ..PciAddress::new(5, 0, 0, 0)
        };
        assert!(ecam.offset(valid).is_some());
        assert!(ecam.offset(PciAddress::new(3, 0, 0, 0)).is_none());
        assert!(ecam.offset(PciAddress::new(9, 0, 0, 0)).is_none());
        let other = PciAddress {
            segment: 0,
            ..PciAddress::new(5, 0, 0, 0)
        };
        assert!(ecam.offset(other).is_none());
    }
}
