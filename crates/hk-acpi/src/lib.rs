//! ACPI 表发现：校验 RSDP，遍历 RSDT/XSDT，取指定签名的表。
//!
//! 调用方提供物理读取函数，把表映射到可读内存；本 crate 不依赖架构。

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

const SIGNATURE_RSDP: &[u8; 8] = b"RSD PTR ";
const SIGNATURE_RSDT: &[u8; 4] = b"RSDT";
const SIGNATURE_XSDT: &[u8; 4] = b"XSDT";
const RSDP_MIN_LENGTH: usize = 20;
const RSDP_V2_EXTRA_LENGTH: usize = 16;
const HEADER_LENGTH: usize = 36;
const MAX_TABLE_LENGTH: usize = 64 * 1024;

/// 从 RSDP 查找指定签名的 ACPI 表。
pub fn find_table<F>(rsdp_phys: u64, signature: &[u8; 4], mut read: F) -> Option<Vec<u8>>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let rsdp = read(rsdp_phys, RSDP_MIN_LENGTH)?;
    if rsdp[0..8] != SIGNATURE_RSDP[..] || !checksum(&rsdp) {
        return None;
    }

    let (root_phys, extended) = if rsdp[15] >= 2 {
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
    if extended {
        if header[0..4] != SIGNATURE_XSDT[..] {
            return None;
        }
    } else if header[0..4] != SIGNATURE_RSDT[..] {
        return None;
    }
    let length = table_length(&header)?;
    let root = read(root_phys, length)?;
    if !checksum(&root) {
        return None;
    }

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
        if header[0..4] != signature[..] {
            continue;
        }
        let length = table_length(&header)?;
        let table = read(table_phys, length)?;
        if checksum(&table) {
            return Some(table);
        }
    }

    None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(bytes: &mut [u8]) {
        bytes[9] = 0;
        let sum = bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[9] = 0_u8.wrapping_sub(sum);
    }

    #[test]
    fn finds_madt_through_rsdt() {
        let mut memory = alloc::vec![0_u8; 0x2000];
        let rsdp = 0x1000_usize;
        let rsdt = 0x1100_usize;
        let apic = 0x1200_usize;

        memory[rsdp..rsdp + 8].copy_from_slice(SIGNATURE_RSDP);
        memory[rsdp + 15] = 0;
        memory[rsdp + 16..rsdp + 20].copy_from_slice(&(rsdt as u32).to_le_bytes());
        memory[rsdp + 9] = 0;
        let sum = memory[rsdp..rsdp + 20]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        memory[rsdp + 9] = 0_u8.wrapping_sub(sum);

        memory[rsdt..rsdt + 4].copy_from_slice(SIGNATURE_RSDT);
        memory[rsdt + 4..rsdt + 8].copy_from_slice(&44_u32.to_le_bytes());
        memory[rsdt + 36..rsdt + 40].copy_from_slice(&(apic as u32).to_le_bytes());
        seal(&mut memory[rsdt..rsdt + 44]);

        memory[apic..apic + 4].copy_from_slice(b"APIC");
        memory[apic + 4..apic + 8].copy_from_slice(&44_u32.to_le_bytes());
        seal(&mut memory[apic..apic + 44]);

        let table = find_table(rsdp as u64, b"APIC", |phys, length| {
            let start = phys as usize;
            let end = start.checked_add(length)?;
            memory.get(start..end).map(|bytes| bytes.to_vec())
        })
        .expect("找到 MADT");
        assert_eq!(&table[0..4], b"APIC");
        assert_eq!(table.len(), 44);
    }
}
