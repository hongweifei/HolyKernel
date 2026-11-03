//! ACPI 表解析：从 RSDP 找到 MADT，取出 I/O APIC 与中断源覆盖。
//!
//! 调用方提供读取函数，把物理表映射到可读内存后拷贝出来；解析过程校验签名与
//! 校验和，越界或校验失败时返回 `None`。

use alloc::vec::Vec;

const SIGNATURE_RSDP: &[u8; 8] = b"RSD PTR ";
const SIGNATURE_MADT: &[u8; 4] = b"APIC";
const RSDP_MIN_LENGTH: usize = 20;
const RSDP_V2_EXTRA_LENGTH: usize = 16;
const HEADER_LENGTH: usize = 36;
const MAX_TABLE_LENGTH: usize = 64 * 1024;
const MAX_OVERRIDES: usize = 8;

/// I/O APIC 与中断源覆盖。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Madt {
    /// I/O APIC 寄存器页物理基址。
    pub ioapic_base: u64,
    /// I/O APIC 的全局系统中断基址。
    pub gsi_base: u32,
    overrides: [(u8, u32); MAX_OVERRIDES],
    override_count: usize,
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
}

/// 从 RSDP 物理地址解析 MADT。
///
/// `read` 读取从物理地址开始的指定长度字节；调用方负责让这段物理内存在当前
/// 页表中可读。
pub fn parse<F>(rsdp_phys: u64, mut read: F) -> Option<Madt>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let rsdp = read(rsdp_phys, RSDP_MIN_LENGTH)?;
    if rsdp[0..8] != SIGNATURE_RSDP[..] || !checksum(&rsdp) {
        return None;
    }

    let revision = rsdp[15];
    let (root_phys, extended) = if revision >= 2 {
        let extra = read(rsdp_phys + RSDP_MIN_LENGTH as u64, RSDP_V2_EXTRA_LENGTH)?;
        let length = u32::from_le_bytes(extra[0..4].try_into().ok()?);
        if (length as usize) < HEADER_LENGTH {
            return None;
        }
        (u64::from_le_bytes(extra[4..12].try_into().ok()?), true)
    } else {
        (
            u64::from(u32::from_le_bytes(rsdp[16..20].try_into().ok()?)),
            false,
        )
    };

    let header = read(root_phys, HEADER_LENGTH)?;
    let length = table_length(&header)?;
    let root = read(root_phys, length)?;

    let entry_size = if extended { 8 } else { 4 };
    let count = root.len().checked_sub(HEADER_LENGTH)? / entry_size;
    for index in 0..count {
        let offset = HEADER_LENGTH + index * entry_size;
        let table_phys = if extended {
            u64::from_le_bytes(root[offset..offset + 8].try_into().ok()?)
        } else {
            u64::from(u32::from_le_bytes(
                root[offset..offset + 4].try_into().ok()?,
            ))
        };
        if table_phys == 0 {
            continue;
        }

        let header = read(table_phys, HEADER_LENGTH)?;
        if header[0..4] == SIGNATURE_MADT[..] {
            let length = table_length(&header)?;
            let bytes = read(table_phys, length)?;
            return parse_madt(&bytes);
        }
    }

    None
}

fn parse_madt(bytes: &[u8]) -> Option<Madt> {
    if bytes.len() < 44 {
        return None;
    }

    let mut ioapic_base = None;
    let mut gsi_base = 0;
    let mut overrides = [(0_u8, 0_u32); MAX_OVERRIDES];
    let mut override_count = 0;

    let mut offset = 44;
    while offset + 2 <= bytes.len() {
        let entry_type = bytes[offset];
        let entry_length = bytes[offset + 1] as usize;
        if entry_length < 2 || offset + entry_length > bytes.len() {
            break;
        }

        match entry_type {
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
    })
}

fn table_length(header: &[u8]) -> Option<usize> {
    let length = u32::from_le_bytes(header[4..8].try_into().ok()?) as usize;
    if !(HEADER_LENGTH..=MAX_TABLE_LENGTH).contains(&length) {
        return None;
    }
    Some(length)
}

fn checksum(bytes: &[u8]) -> bool {
    bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}
