//! x86_64 页表映射与枚举测试。

use hk_hal::{PAGE_SIZE, PhysAddr, VirtAddr};
use hk_mem::{FrameBank, FrameKind};

use crate::paging::flags::{HUGE, NX, PRESENT, PageFlags, USER, WRITABLE};

use super::PageTable;

#[test]
fn unmaps_absent_range_idempotently() {
    let mut memory = alloc::vec![0_u8; PAGE_SIZE];
    let base = memory.as_mut_ptr() as u64;
    let table = PageTable {
        direct_map: base,
        root: PhysAddr::new(0),
        pcid: 0,
    };
    assert!(table.unmap_user_range(VirtAddr::new(0x1000), PAGE_SIZE));
    assert!(!table.unmap_user_range(VirtAddr::new(0x1001), PAGE_SIZE));
}

#[test]
fn maps_2m_huge_page() {
    let mut memory = alloc::vec![0_u8; 3 * PAGE_SIZE];
    let base = memory.as_mut_ptr() as u64;
    let table = PageTable {
        direct_map: base,
        root: PhysAddr::new(0),
        pcid: 0,
    };
    let allocator = FrameBank::from_range(PAGE_SIZE as u64, 3 * PAGE_SIZE as u64);
    table
        .map_huge_with(
            VirtAddr::new(0x20_0000),
            PhysAddr::new(0x40_0000),
            PageFlags::from_raw(PRESENT | WRITABLE | USER),
            1 << 21,
            || allocator.allocate_for(FrameKind::PageTable),
        )
        .expect("建立 2 MiB 大页");
    let (phys, flags) = table
        .translate(VirtAddr::new(0x20_0000 + 0x1234))
        .expect("翻译大页");
    assert_eq!(phys, PhysAddr::new(0x40_0000 + 0x1234));
    assert_ne!(flags & HUGE, 0);
}

#[test]
fn creates_user_table_with_kernel_entries() {
    let mut memory = alloc::vec![0_u8; 3 * PAGE_SIZE];
    let base = memory.as_mut_ptr() as u64;
    // SAFETY: 缓冲区覆盖模板与用户页表各一页。
    unsafe {
        let root = base as *mut u64;
        root.add(0).write_volatile(0x9ABC);
        root.add(256).write_volatile(0x1234);
        root.add(511).write_volatile(0x5678);
    }

    let table = PageTable {
        direct_map: base,
        root: PhysAddr::new(0),
        pcid: 0,
    };
    let allocator = FrameBank::from_range(PAGE_SIZE as u64, 2 * PAGE_SIZE as u64);
    let user = table.create_user(&allocator).expect("创建用户页表");
    assert_eq!(user.root, PhysAddr::new(PAGE_SIZE as u64));

    // SAFETY: 用户页表位于缓冲区第二页，已清零并复制内核入口。
    unsafe {
        let root = (base + PAGE_SIZE as u64) as *const u64;
        assert_eq!(root.add(0).read_volatile(), 0);
        assert_eq!(root.add(256).read_volatile(), 0x1234);
        assert_eq!(root.add(511).read_volatile(), 0x5678);
    }
}

#[test]
fn kernel_half_regions_walks_high_mappings() {
    // 缓冲区依次容纳 PML4、PDPT、PD、PT 与两个数据页。
    let mut memory = alloc::vec![0_u8; 6 * PAGE_SIZE];
    let base = memory.as_mut_ptr() as u64;
    let table = PageTable {
        direct_map: base,
        root: PhysAddr::new(0),
        pcid: 0,
    };
    let pml4 = 0_u64;
    let pdpt = PAGE_SIZE as u64;
    let pd = 2 * PAGE_SIZE as u64;
    let pt = 3 * PAGE_SIZE as u64;
    let data_a = 4 * PAGE_SIZE as u64;
    let data_b = 5 * PAGE_SIZE as u64;
    // 高半区虚拟地址：PML4[256] → PDPT[0] → PD[0] → PT[0..2]。
    let virt = 0xFFFF_8000_0000_0000_u64;
    // SAFETY: 各级页表与数据页都在缓冲区内，索引在 0..512 范围内。
    unsafe {
        table
            .entry(PhysAddr::new(pml4), 256)
            .write_volatile(pdpt | PRESENT | WRITABLE | USER);
        table
            .entry(PhysAddr::new(pdpt), 0)
            .write_volatile(pd | PRESENT | WRITABLE | USER);
        table
            .entry(PhysAddr::new(pd), 0)
            .write_volatile(pt | PRESENT | WRITABLE | USER);
        table
            .entry(PhysAddr::new(pt), 0)
            .write_volatile(data_a | PRESENT | WRITABLE);
        table
            .entry(PhysAddr::new(pt), 1)
            .write_volatile(data_b | PRESENT | WRITABLE | NX);
    }
    let mut regions = alloc::vec::Vec::new();
    table.kernel_half_regions(|virt, phys, len, flags| {
        regions.push((virt, phys, len, flags));
    });
    assert_eq!(
        regions,
        alloc::vec![
            (virt, data_a, PAGE_SIZE as u64, PRESENT | WRITABLE),
            (
                virt + PAGE_SIZE as u64,
                data_b,
                PAGE_SIZE as u64,
                PRESENT | WRITABLE | NX
            ),
        ]
    );
}
