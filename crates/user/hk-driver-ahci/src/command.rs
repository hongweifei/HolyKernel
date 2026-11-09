//! AHCI 命令 FIS / PRDT 布局。

use crate::dma::SLOT_BYTES;

const PRDT_OFFSET: usize = 0x80;
const PRDT_ENTRY_BYTES: usize = 16;
const MAX_PRDT: usize = (SLOT_BYTES - PRDT_OFFSET) / PRDT_ENTRY_BYTES;

/// ATA IDENTIFY DEVICE。
pub const ATA_IDENTIFY: u8 = 0xEC;
/// ATA READ DMA EXT。
pub const ATA_READ_DMA_EXT: u8 = 0x25;
/// ATA WRITE DMA EXT。
pub const ATA_WRITE_DMA_EXT: u8 = 0x35;

/// 构造 48 位 LBA 的 Host-to-Device Register FIS。
pub fn cfis(command: u8, lba: u64, sectors: u16) -> [u8; 20] {
    let mut fis = [0_u8; 20];
    fis[0] = 0x27;
    fis[1] = 0x80;
    fis[2] = command;
    fis[4] = lba as u8;
    fis[5] = (lba >> 8) as u8;
    fis[6] = (lba >> 16) as u8;
    fis[7] = 0x40;
    fis[8] = (lba >> 24) as u8;
    fis[9] = (lba >> 32) as u8;
    fis[10] = (lba >> 40) as u8;
    fis[12] = sectors as u8;
    fis[13] = (sectors >> 8) as u8;
    fis
}

/// 在指定槽写入命令头、CFIS 与多 PRDT。
///
/// `entry_virt` 是命令头地址，`table_virt/table_phys` 是命令表地址；
/// 调用方保证都在本驱动的 DMA 能力内。
pub fn install(
    entry_virt: u64,
    table_virt: u64,
    table_phys: u64,
    fis: &[u8; 20],
    data_phys: u64,
    bytes: usize,
) -> Option<u16> {
    if bytes == 0 {
        return None;
    }
    let entries = bytes.div_ceil(4096);
    if entries > MAX_PRDT {
        return None;
    }
    // SAFETY: 命令头与命令表都属于本驱动的 DMA 能力。
    unsafe {
        core::ptr::write_bytes(entry_virt as *mut u8, 0, 32);
        core::ptr::write_bytes(table_virt as *mut u8, 0, SLOT_BYTES);
        core::ptr::copy_nonoverlapping(fis.as_ptr(), table_virt as *mut u8, fis.len());
    }
    let mut offset = 0_usize;
    for index in 0..entries {
        let chunk = (bytes - offset).min(4096);
        let prdt = table_virt + PRDT_OFFSET as u64 + (index * PRDT_ENTRY_BYTES) as u64;
        // SAFETY: PRDT 位于本槽命令表，索引由 entries 上限保证。
        unsafe {
            core::ptr::write_volatile(prdt as *mut u64, data_phys + offset as u64);
            let flags = (chunk as u32 - 1) | if offset + chunk == bytes { 1 << 31 } else { 0 };
            core::ptr::write_volatile((prdt as *mut u32).add(2), flags);
        }
        offset += chunk;
    }
    // SAFETY: 命令头位于命令列表，写 CFL、PRDTL 与命令表物理地址。
    unsafe {
        core::ptr::write_volatile(entry_virt as *mut u32, 5 | ((entries as u32) << 16));
        core::ptr::write_volatile((entry_virt as *mut u64).add(1), table_phys);
    }
    Some(entries as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfis_encodes_48_bit_lba() {
        let fis = cfis(ATA_READ_DMA_EXT, 0x1234_5678_9ABC, 0x0102);
        assert_eq!(fis[0], 0x27);
        assert_eq!(fis[1], 0x80);
        assert_eq!(fis[2], ATA_READ_DMA_EXT);
        assert_eq!(&fis[4..7], &[0xBC, 0x9A, 0x78]);
        assert_eq!(fis[7], 0x40);
        assert_eq!(&fis[8..11], &[0x56, 0x34, 0x12]);
        assert_eq!(fis[12], 0x02);
        assert_eq!(fis[13], 0x01);
    }

    #[test]
    fn installs_header_and_multi_entry_prdt() {
        let mut entry = alloc::vec![0_u8; 32];
        let mut table = alloc::vec![0_u8; SLOT_BYTES];
        let fis = cfis(ATA_WRITE_DMA_EXT, 0x100, 8);
        // SAFETY: 两个缓冲覆盖命令头与命令表，长度均满足 install 的要求。
        install(
            entry.as_mut_ptr() as u64,
            table.as_mut_ptr() as u64,
            0x20_0000,
            &fis,
            0x10_0000,
            9000,
        )
        .expect("install");
        assert_eq!(
            u32::from_le_bytes(entry[0..4].try_into().unwrap()),
            5 | (3 << 16)
        );
        assert_eq!(
            u64::from_le_bytes(entry[8..16].try_into().unwrap()),
            0x20_0000
        );
        let prdt = |index: usize| {
            let at = 0x80 + index * 16;
            (
                u64::from_le_bytes(table[at..at + 8].try_into().unwrap()),
                u32::from_le_bytes(table[at + 8..at + 12].try_into().unwrap()),
            )
        };
        assert_eq!(prdt(0), (0x10_0000, 4095));
        assert_eq!(prdt(1), (0x10_1000, 4095));
        assert_eq!(prdt(2).0, 0x10_2000);
        assert_eq!(prdt(2).1, 807 | (1 << 31));
    }
}
