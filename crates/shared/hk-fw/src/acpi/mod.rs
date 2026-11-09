//! ACPI 表解析。
//!
//! RSDP/RSDT/XSDT 的物理内存遍历由本模块内部的 `rsdp` 实现；
//! 表结构本身由外部 `acpi` crate 解释。外部类型不出现在本模块的公开 API 中。

pub(crate) mod rsdp;

use alloc::vec::Vec;
use core::pin::Pin;

use acpi::sdt::madt::{Madt as AcpiMadt, MadtEntry};

/// MADT 签名。
const SIGNATURE_MADT: &[u8; 4] = b"APIC";

/// I/O APIC 描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApic {
    /// I/O APIC 编号。
    pub id: u8,
    /// 寄存器页物理基址。
    pub address: u64,
    /// 全局系统中断基址。
    pub gsi_base: u32,
}

/// ISA IRQ 到全局系统中断的覆盖。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqOverride {
    /// 总线相对 IRQ。
    pub source: u8,
    /// 对应的全局系统中断。
    pub gsi: u32,
}

/// MADT 中的处理器条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Processor {
    /// ACPI 处理器 UID。
    pub acpi_id: u32,
    /// 本地 APIC/x2APIC 编号。
    pub apic_id: u32,
    /// 是否已启用。
    pub enabled: bool,
}

/// 多处理器中断控制器表（MADT）的架构无关视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Madt {
    /// 本地 APIC 寄存器页物理基址。
    pub local_apic_address: u64,
    /// 检测到的 I/O APIC。
    pub io_apics: Vec<IoApic>,
    /// 已启用的本地 APIC 编号。
    pub local_apics: Vec<u32>,
    /// 处理器条目（ACPI UID 与 APIC 编号）。
    pub processors: Vec<Processor>,
    /// 中断源覆盖。
    pub overrides: Vec<IrqOverride>,
    /// ARM GIC 分配器基址。
    pub gic_distributors: Vec<u64>,
    /// ARM GIC CPU 接口基址。
    pub gic_cpu_interfaces: Vec<u64>,
}

impl Madt {
    /// 返回第一个 I/O APIC 的寄存器页物理基址。
    pub fn ioapic_base(&self) -> Option<u64> {
        self.io_apics.first().map(|ioapic| ioapic.address)
    }

    /// 返回第一个 I/O APIC 的全局系统中断基址。
    pub fn gsi_base(&self) -> u32 {
        self.io_apics.first().map_or(0, |ioapic| ioapic.gsi_base)
    }

    /// 返回 ISA IRQ 对应的全局系统中断号，遵循中断源覆盖。
    pub fn gsi(&self, irq: u8) -> u32 {
        if let Some(entry) = self.overrides.iter().find(|entry| entry.source == irq) {
            return entry.gsi;
        }
        self.gsi_base().saturating_add(u32::from(irq))
    }

    /// 返回已启用的本地 APIC 编号。
    pub fn local_apics(&self) -> &[u32] {
        &self.local_apics
    }

    /// 通过 APIC 编号查找处理器条目。
    pub fn processor_for_apic(&self, apic_id: u32) -> Option<&Processor> {
        self.processors
            .iter()
            .find(|entry| entry.apic_id == apic_id)
    }

    /// 返回检测到的 CPU 数量。
    pub fn cpu_count(&self) -> usize {
        self.local_apics.len()
    }

    /// 返回第一个 GIC 分配器基址。
    pub fn gic_distributor(&self) -> Option<u64> {
        self.gic_distributors.first().copied()
    }
}

/// 从 RSDP 物理地址解析 MADT。
///
/// `read` 读取从物理地址开始的指定长度字节；调用方负责让这段物理内存在当前
/// 页表中可读。
pub fn parse<F>(rsdp_phys: u64, mut read: F) -> Option<Madt>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let bytes = rsdp::find_table(rsdp_phys, SIGNATURE_MADT, &mut read)?;
    parse_table(&bytes)
}

/// 从已读取的 MADT 字节解析。
fn parse_table(bytes: &[u8]) -> Option<Madt> {
    let header = core::mem::size_of::<AcpiMadt>();
    if bytes.len() < header {
        return None;
    }
    let length = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
    if length < header || length > bytes.len() {
        return None;
    }
    if &bytes[0..4] != SIGNATURE_MADT {
        return None;
    }

    // SAFETY: `AcpiMadt` 是 `repr(C, packed)`（对齐 1），且长度已覆盖其静态部分；
    // 表字节在本次解析期间保持不变。`Madt` 因 `PhantomPinned` 要求不被移动。
    let table = unsafe { &*(bytes.as_ptr() as *const AcpiMadt) };

    let mut madt = Madt {
        local_apic_address: u64::from(table.local_apic_address),
        io_apics: Vec::new(),
        local_apics: Vec::new(),
        processors: Vec::new(),
        overrides: Vec::new(),
        gic_distributors: Vec::new(),
        gic_cpu_interfaces: Vec::new(),
    };

    // SAFETY: `table` 指向 `bytes` 内的 MADT，`bytes` 在迭代期间保持不变且不会移动。
    let pinned = unsafe { Pin::new_unchecked(table) };
    for entry in pinned.entries() {
        match entry {
            MadtEntry::LocalApic(entry) => {
                let flags = entry.flags;
                let apic = u32::from(entry.apic_id);
                let processor = Processor {
                    acpi_id: u32::from(entry.processor_id),
                    apic_id: apic,
                    enabled: flags & 1 != 0,
                };
                push_processor(&mut madt.processors, processor);
                if flags & 1 != 0 {
                    push_unique(&mut madt.local_apics, apic);
                }
            }
            MadtEntry::LocalX2Apic(entry) => {
                let flags = entry.flags;
                let apic = entry.x2apic_id;
                let processor = Processor {
                    acpi_id: entry.processor_uid,
                    apic_id: apic,
                    enabled: flags & 1 != 0,
                };
                push_processor(&mut madt.processors, processor);
                if flags & 1 != 0 {
                    push_unique(&mut madt.local_apics, apic);
                }
            }
            MadtEntry::IoApic(entry) => madt.io_apics.push(IoApic {
                id: entry.io_apic_id,
                address: u64::from(entry.io_apic_address),
                gsi_base: entry.global_system_interrupt_base,
            }),
            MadtEntry::InterruptSourceOverride(entry) => madt.overrides.push(IrqOverride {
                source: entry.irq,
                gsi: entry.global_system_interrupt,
            }),
            MadtEntry::LocalApicAddressOverride(entry) => {
                madt.local_apic_address = entry.local_apic_address;
            }
            MadtEntry::Gicd(entry) => madt.gic_distributors.push(entry.physical_base_address),
            MadtEntry::Gicc(entry) => madt.gic_cpu_interfaces.push(entry.gic_registers_address),
            _ => {}
        }
    }

    Some(madt)
}

fn push_unique(values: &mut Vec<u32>, value: u32) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn push_processor(values: &mut Vec<Processor>, value: Processor) {
    if !values.iter().any(|entry| entry.apic_id == value.apic_id) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[&[u8]]) -> Vec<u8> {
        let header = core::mem::size_of::<AcpiMadt>();
        let mut bytes = alloc::vec![0_u8; header];
        bytes[0..4].copy_from_slice(SIGNATURE_MADT);
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        let length = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        bytes
    }

    #[test]
    fn collects_local_apics_and_overrides() {
        let ioapic = [1, 12, 0, 0, 0x00, 0x00, 0xC0, 0xFE, 0, 0, 0, 0];
        let irq_override = [2, 10, 0, 1, 9, 0, 0, 0, 0, 0];
        let apic0 = [0, 8, 0, 0, 1, 0, 0, 0];
        let apic1 = [0, 8, 1, 1, 1, 0, 0, 0];
        let x2apic = [9, 16, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];

        let madt = parse_table(&table(&[&ioapic, &irq_override, &apic0, &apic1, &x2apic]))
            .expect("解析成功");

        assert_eq!(madt.ioapic_base(), Some(0xFEC0_0000));
        assert_eq!(madt.gsi(1), 9);
        assert_eq!(madt.gsi(3), 3);
        assert_eq!(madt.cpu_count(), 3);
        assert_eq!(madt.local_apics(), &[0, 1, 2]);
    }

    #[test]
    fn rejects_short_table() {
        assert!(parse_table(&[0_u8; 8]).is_none());
    }
}
