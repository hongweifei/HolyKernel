//! x86_64 ACPI MADT 解析：I/O APIC、本地 APIC 与中断源覆盖。
//!
//! RSDP/SDT 查找由 `hk-acpi` 完成；本模块只解释 x86 相关条目。

use alloc::vec::Vec;

const SIGNATURE_MADT: &[u8; 4] = b"APIC";
const MAX_OVERRIDES: usize = 8;
const MAX_CPUS: usize = 16;

/// I/O APIC、中断源覆盖与本地 APIC 列表。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Madt {
    /// I/O APIC 寄存器页物理基址。
    pub ioapic_base: u64,
    /// I/O APIC 的全局系统中断基址。
    pub gsi_base: u32,
    overrides: [(u8, u32); MAX_OVERRIDES],
    override_count: usize,
    local_apics: [u32; MAX_CPUS],
    local_count: usize,
}

impl Madt {
    /// 返回 ISA IRQ 对应的全局系统中断号，遵循中断源覆盖。
    pub fn gsi(&self, irq: u8) -> u32 {
        for (source, gsi) in &self.overrides[..self.override_count] {
            if *source == irq {
                return *gsi;
            }
        }
        self.gsi_base + u32::from(irq)
    }

    /// 返回启用的本地 APIC 编号。
    pub fn local_apics(&self) -> &[u32] {
        &self.local_apics[..self.local_count]
    }

    /// 返回检测到的 CPU 数量。
    pub fn cpu_count(&self) -> usize {
        self.local_count
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
    let bytes = hk_acpi::find_table(rsdp_phys, SIGNATURE_MADT, &mut read)?;
    parse_madt(&bytes)
}

fn parse_madt(bytes: &[u8]) -> Option<Madt> {
    if bytes.len() < 44 {
        return None;
    }

    let mut ioapic_base = None;
    let mut gsi_base = 0;
    let mut overrides = [(0_u8, 0_u32); MAX_OVERRIDES];
    let mut override_count = 0;
    let mut local_apics = [0_u32; MAX_CPUS];
    let mut local_count = 0;

    let mut offset = 44;
    while offset + 2 <= bytes.len() {
        let entry_type = bytes[offset];
        let entry_length = bytes[offset + 1] as usize;
        if entry_length < 2 || offset + entry_length > bytes.len() {
            break;
        }

        match entry_type {
            // 处理器本地 APIC：ACPI ID、APIC ID、标志。
            0 if entry_length >= 8 => {
                let flags = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
                let apic = u32::from(bytes[offset + 3]);
                if flags & 1 != 0
                    && local_count < MAX_CPUS
                    && !local_apics[..local_count].contains(&apic)
                {
                    local_apics[local_count] = apic;
                    local_count += 1;
                }
            }
            // x2APIC：APIC ID、标志、UID。
            9 if entry_length >= 16 => {
                let apic = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
                let flags = u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().ok()?);
                if flags & 1 != 0
                    && local_count < MAX_CPUS
                    && !local_apics[..local_count].contains(&apic)
                {
                    local_apics[local_count] = apic;
                    local_count += 1;
                }
            }
            // I/O APIC：id、保留、地址、GSI 基址。
            1 if entry_length >= 12 => {
                let address = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
                ioapic_base = Some(u64::from(address));
                gsi_base = u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().ok()?);
            }
            // 中断源覆盖：总线、源 IRQ、GSI、标志。
            2 if entry_length >= 10 && override_count < MAX_OVERRIDES => {
                let source = bytes[offset + 3];
                let gsi = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
                overrides[override_count] = (source, gsi);
                override_count += 1;
            }
            _ => {}
        }

        offset += entry_length;
    }

    Some(Madt {
        ioapic_base: ioapic_base?,
        gsi_base,
        overrides,
        override_count,
        local_apics,
        local_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[&[u8]]) -> Vec<u8> {
        let mut bytes = alloc::vec![0_u8; 44];
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }

    #[test]
    fn collects_local_apics_and_overrides() {
        let ioapic = [1, 12, 0, 0, 0x00, 0x00, 0xC0, 0xFE, 0, 0, 0, 0];
        let irq_override = [2, 10, 0, 1, 9, 0, 0, 0, 0, 0];
        let apic0 = [0, 8, 0, 0, 1, 0, 0, 0];
        let apic1 = [0, 8, 1, 1, 1, 0, 0, 0];
        let x2apic = [9, 16, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];

        let madt = parse_madt(&table(&[&ioapic, &irq_override, &apic0, &apic1, &x2apic]))
            .expect("解析成功");

        assert_eq!(madt.ioapic_base, 0xFEC0_0000);
        assert_eq!(madt.gsi(1), 9);
        assert_eq!(madt.cpu_count(), 3);
        assert_eq!(madt.local_apics(), &[0, 1, 2]);
    }
}
