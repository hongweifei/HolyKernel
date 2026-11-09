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
const MAX_TABLE_LENGTH: usize = 1024 * 1024;

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
        // RSDP v2 的扩展校验和覆盖完整的 36 字节。
        let sum = rsdp
            .iter()
            .chain(extra.iter())
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        if sum != 0 {
            return None;
        }
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

/// 从 RSDP 查找指定签名的全部 ACPI 表。
pub fn find_tables<F>(rsdp_phys: u64, signature: &[u8; 4], mut read: F) -> Vec<Vec<u8>>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let Some(rsdp) = read(rsdp_phys, RSDP_MIN_LENGTH) else {
        return Vec::new();
    };
    if rsdp[0..8] != SIGNATURE_RSDP[..] || !checksum(&rsdp) {
        return Vec::new();
    }
    let (root_phys, extended) = if rsdp[15] >= 2 {
        let Some(extra) = read(rsdp_phys + RSDP_MIN_LENGTH as u64, RSDP_V2_EXTRA_LENGTH) else {
            return Vec::new();
        };
        let sum = rsdp
            .iter()
            .chain(extra.iter())
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        if sum != 0 {
            return Vec::new();
        }
        let Some(length) = u32_at(&extra, 0) else {
            return Vec::new();
        };
        if (length as usize) < HEADER_LENGTH {
            return Vec::new();
        }
        (
            u64::from_le_bytes(extra[4..12].try_into().unwrap_or_default()),
            true,
        )
    } else {
        (
            u64::from(u32::from_le_bytes(
                rsdp[16..20].try_into().unwrap_or_default(),
            )),
            false,
        )
    };
    let Some(header) = read(root_phys, HEADER_LENGTH) else {
        return Vec::new();
    };
    let root_signature = if extended {
        SIGNATURE_XSDT
    } else {
        SIGNATURE_RSDT
    };
    if header[0..4] != root_signature[..] {
        return Vec::new();
    }
    let Some(length) = table_length(&header) else {
        return Vec::new();
    };
    let Some(root) = read(root_phys, length) else {
        return Vec::new();
    };
    if !checksum(&root) {
        return Vec::new();
    }
    let entry_size = if extended { 8 } else { 4 };
    let Some(count) = root
        .len()
        .checked_sub(HEADER_LENGTH)
        .map(|len| len / entry_size)
    else {
        return Vec::new();
    };
    let mut tables = Vec::new();
    for index in 0..count {
        let offset = HEADER_LENGTH + index * entry_size;
        let table_phys = if extended {
            u64::from_le_bytes(root[offset..offset + 8].try_into().unwrap_or_default())
        } else {
            u64::from(u32::from_le_bytes(
                root[offset..offset + 4].try_into().unwrap_or_default(),
            ))
        };
        if table_phys == 0 {
            continue;
        }
        let Some(header) = read(table_phys, HEADER_LENGTH) else {
            continue;
        };
        if header[0..4] != signature[..] {
            continue;
        }
        let Some(length) = table_length(&header) else {
            continue;
        };
        let Some(table) = read(table_phys, length) else {
            continue;
        };
        if checksum(&table) {
            tables.push(table);
        }
    }
    tables
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

/// 读取并按 ACPI 表头校验一张表（不含 RSDT/XSDT 遍历）。
pub fn read_table<F>(phys: u64, mut read: F) -> Option<Vec<u8>>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let header = read(phys, HEADER_LENGTH)?;
    let length = table_length(&header)?;
    let table = read(phys, length)?;
    checksum(&table).then_some(table)
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

/// 遍历起始偏移之后的 ACPI 表条目。
pub struct Entries<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Entries<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + 2 > self.bytes.len() {
            return None;
        }
        let entry_type = self.bytes[self.offset];
        let length = self.bytes[self.offset + 1] as usize;
        if length < 2 || self.offset + length > self.bytes.len() {
            self.offset = self.bytes.len();
            return None;
        }
        let entry = &self.bytes[self.offset..self.offset + length];
        self.offset += length;
        Some((entry_type, entry))
    }
}

/// 从 `start` 开始遍历 ACPI 表条目；越界或长度非法时结束。
pub fn entries(table: &[u8], start: usize) -> Entries<'_> {
    Entries {
        bytes: table,
        offset: start,
    }
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
    fn iterates_table_entries() {
        let table = [
            0, 8, 0, 1, 1, 0, 0, 0, // 类型 0，长度 8
            9, 16, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, // 类型 9，长度 16
        ];
        let entries: alloc::vec::Vec<_> = entries(&table, 0).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, 0);
        assert_eq!(entries[0].1.len(), 8);
        assert_eq!(entries[1].0, 9);
        assert_eq!(entries[1].1.len(), 16);
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
