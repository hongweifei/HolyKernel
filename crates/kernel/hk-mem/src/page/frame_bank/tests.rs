use super::*;
use crate::hmp::{HmpError, PhysicalBackend};
use hk_hal::RegionKind;

#[test]
fn capacity_ledger_enforces_owner_quota() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let owner = CapacityId::next();
    bank.reserve_for(owner, 0x2000).expect("预留");
    bank.register_ledger(owner, PAGE_SIZE as u64);
    assert!(
        bank.allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
            .is_some()
    );
    assert!(
        bank.allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
            .is_none(),
        "超出容量 Ledger 配额应拒绝"
    );
    assert_eq!(bank.used_owned(owner), PAGE_SIZE as u64);
}

#[test]
fn adopting_owner_charges_its_ledger() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let owner = CapacityId::next();
    bank.register_ledger(owner, 0x4000);
    let range = bank.allocate_contiguous(PAGE_SIZE).expect("内核自用页");
    assert_eq!(bank.used_owned(owner), 0);
    assert!(bank.adopt_owner(owner, range));
    assert_eq!(bank.used_owned(owner), PAGE_SIZE as u64);
    assert!(bank.release_owned(owner, range));
    assert_eq!(bank.used_owned(owner), 0);
}

#[test]
fn covers_all_usable_regions() {
    let regions = [
        MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2_0000, RegionKind::Usable),
        MemoryRegion::new(PhysAddr::new(0x10_0000), 0x40_0000, RegionKind::Usable),
        MemoryRegion::new(PhysAddr::new(0x80_0000), 0x10_0000, RegionKind::Reserved),
    ];
    let kernel = MemoryRegion::new(PhysAddr::new(0), 0x1000, RegionKind::Kernel);
    let bank = FrameBank::empty();
    bank.init(&regions, kernel);
    assert_eq!(bank.allocate(), Some(PhysAddr::new(0x1_0000)));
    let large = bank.allocate_contiguous(0x20_000).expect("连续页");
    assert_eq!(large.start(), PhysAddr::new(0x10_0000));
}

#[test]
fn avoids_kernel_image() {
    let regions = [MemoryRegion::new(
        PhysAddr::new(0x1_0000),
        0x10_0000,
        RegionKind::Usable,
    )];
    let kernel = MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2000, RegionKind::Kernel);
    let bank = FrameBank::empty();
    bank.init(&regions, kernel);
    assert_eq!(bank.allocate(), Some(PhysAddr::new(0x1_2000)));
}

#[test]
fn allocates_and_coalesces_frames() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_3000);
    let range = bank.allocate_contiguous(0x1800).expect("连续分配");
    assert_eq!(range.start(), PhysAddr::new(0x10_0000));
    assert_eq!(range.len(), 0x2000);
    assert_eq!(bank.allocate(), Some(PhysAddr::new(0x10_2000)));
    assert_eq!(bank.allocate_contiguous(0x2000), None);
    bank.free(range);
    bank.free(PhysRange::new(PhysAddr::new(0x10_2000), 0x1000));
    let merged = bank.allocate_contiguous(0x3000).expect("合并后");
    assert_eq!(merged.start(), PhysAddr::new(0x10_0000));
}

#[test]
fn reserved_window_only_serves_its_owner() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let owner = CapacityId::next();
    let reserved = bank.reserve_for(owner, 0x2000).expect("预留");
    assert_eq!(reserved.start(), PhysAddr::new(0x10_0000));

    // 未归属的分配跳过预留窗口，从其后取页。
    let unowned = bank.allocate().expect("内核页");
    assert_eq!(unowned, PhysAddr::new(0x10_2000));

    // 归属分配只能从预留窗口内取页。
    let owned = bank
        .allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
        .expect("归属页");
    assert_eq!(owned.start(), PhysAddr::new(0x10_0000));
    assert_eq!(bank.used_owned(owner), PAGE_SIZE as u64);

    // 再次归属分配取窗口内下一页。
    let second = bank
        .allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
        .expect("归属第二页");
    assert_eq!(second.start(), PhysAddr::new(0x10_1000));
    assert_eq!(bank.used_owned(owner), 2 * PAGE_SIZE as u64);

    // 窗口耗尽后拒绝。
    assert!(
        bank.allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
            .is_none()
    );
}

#[test]
fn released_owned_range_returns_to_window() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_2000);
    let owner = CapacityId::next();
    bank.reserve_for(owner, 0x2000).expect("预留");
    let range = bank
        .allocate_owned(owner, CapacityKind::Dma, PAGE_SIZE as u64)
        .expect("归属页");
    assert_eq!(bank.used_owned(owner), PAGE_SIZE as u64);
    assert_eq!(bank.peak_owned(owner), PAGE_SIZE as u64);

    assert!(bank.release_owned(owner, range));
    assert_eq!(bank.used_owned(owner), 0);
    assert_eq!(bank.peak_owned(owner), PAGE_SIZE as u64, "峰值不随释放回落");

    let reused = bank
        .allocate_owned(owner, CapacityKind::Dma, PAGE_SIZE as u64)
        .expect("复用");
    assert_eq!(reused.start(), range.start());
}

#[test]
fn split_carves_child_window_from_parent() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let parent = CapacityId::next();
    let child = CapacityId::next();
    bank.reserve_for(parent, 0x4000).expect("预留父窗口");

    let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");
    assert_eq!(child_range.start(), PhysAddr::new(0x10_2000));

    let child_page = bank
        .allocate_owned(child, CapacityKind::User, PAGE_SIZE as u64)
        .expect("子分配");
    assert_eq!(child_page.start(), PhysAddr::new(0x10_2000));
    assert_eq!(bank.used_owned(child), PAGE_SIZE as u64);
    assert_eq!(bank.used_owned(parent), 0);

    let parent_page = bank
        .allocate_owned(parent, CapacityKind::User, PAGE_SIZE as u64)
        .expect("父分配");
    assert_eq!(parent_page.start(), PhysAddr::new(0x10_0000));
}

#[test]
fn reserve_rejects_second_window_for_same_owner() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let owner = CapacityId::next();
    bank.reserve_for(owner, 0x2000).expect("首次预留");
    assert!(
        bank.reserve_for(owner, 0x2000).is_none(),
        "同一容量不允许预留第二个窗口"
    );
    // 另一个容量仍可预留。
    let other = CapacityId::next();
    assert!(bank.reserve_for(other, 0x2000).is_some());
}

#[test]
fn merge_rejoins_adjacent_child_window() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let parent = CapacityId::next();
    let child = CapacityId::next();
    bank.reserve_for(parent, 0x4000).expect("预留父窗口");
    let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");

    bank.merge_owned(parent, child, child_range)
        .expect("并回子窗口");
    // 子窗口并回父窗口后，父容量可以分配整段 0x4000。
    let whole = bank
        .allocate_owned(parent, CapacityKind::User, 0x4000)
        .expect("父容量应能取回整段");
    assert_eq!(whole.start(), PhysAddr::new(0x10_0000));
    assert_eq!(whole.len(), 0x4000);
}

#[test]
fn merge_keeps_non_adjacent_window_separate() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_8000);
    let parent = CapacityId::next();
    let first = CapacityId::next();
    let second = CapacityId::next();
    bank.reserve_for(parent, 0x8000).expect("预留父窗口");

    // 先切出后段给 second，再切出中段给 first，使它们不相邻。
    let second_range = bank.split_owned(parent, second, 0x2000).expect("切后段");
    let first_range = bank.split_owned(parent, first, 0x2000).expect("切中段");
    assert_eq!(second_range.start(), PhysAddr::new(0x10_6000));
    assert_eq!(first_range.start(), PhysAddr::new(0x10_4000));

    // 父窗口是 0x10_0000..0x10_4000，first 紧邻其后 → 相邻合并。
    bank.merge_owned(parent, first, first_range)
        .expect("并回相邻子窗口");
    let page = bank
        .allocate_owned(parent, CapacityKind::User, 0x4000)
        .expect("相邻合并后父容量可取 0x4000");
    assert_eq!(page.start(), PhysAddr::new(0x10_0000));

    // second 之后并入父容量，不再归 second 所有。
    bank.merge_owned(parent, second, second_range)
        .expect("并回 second");
    assert!(
        bank.allocate_owned(second, CapacityKind::User, PAGE_SIZE as u64)
            .is_none(),
        "并回后不再归 second 所有"
    );
}

#[test]
fn merge_owned_rejects_child_with_live_allocations() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let parent = CapacityId::next();
    let child = CapacityId::next();
    bank.reserve_for(parent, 0x4000).expect("预留父窗口");
    let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");
    let page = bank
        .allocate_owned(child, CapacityKind::User, PAGE_SIZE as u64)
        .expect("子页");

    assert_eq!(
        bank.merge_owned(parent, child, child_range),
        Err(HmpError::PermissionDenied),
        "子容量仍有分配时后端拒绝合并"
    );
    // 拒绝后窗口与分配都保留在子容量名下。
    assert_eq!(bank.used_owned(child), PAGE_SIZE as u64);
    assert!(bank.release_owned(child, page));
    assert_eq!(bank.used_owned(child), 0);
}

#[test]
fn merge_owned_rejects_window_of_other_capacity() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let parent = CapacityId::next();
    let child = CapacityId::next();
    let other = CapacityId::next();
    bank.reserve_for(parent, 0x4000).expect("预留父窗口");
    let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");

    assert_eq!(
        bank.merge_owned(parent, other, child_range),
        Err(HmpError::WrongCapability),
        "窗口归属与传入容量不一致时拒绝"
    );
    // 窗口仍在 child 名下。
    assert!(
        bank.allocate_owned(child, CapacityKind::User, PAGE_SIZE as u64)
            .is_some()
    );
}

#[test]
fn merge_owned_is_idempotent_without_window() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_2000);
    let range = PhysRange::new(PhysAddr::new(0x10_0000), 0x1000);
    assert_eq!(
        bank.merge_owned(CapacityId::next(), CapacityId::next(), range),
        Ok(()),
        "没有归属窗口时合并没有副作用"
    );
}

#[test]
fn split_owned_uses_window_whose_tail_is_free() {
    let bank = FrameBank::from_range(0x10_0000, 0x11_0000);
    let parent = CapacityId::next();
    let first = CapacityId::next();
    let second = CapacityId::next();

    bank.reserve_for(parent, 0x1_0000).expect("预留父窗口");
    let second_range = bank.split_owned(parent, second, 0x2000).expect("切后段");
    let first_range = bank.split_owned(parent, first, 0x2000).expect("切中段");
    assert_eq!(second_range.start(), PhysAddr::new(0x10_E000));
    assert_eq!(first_range.start(), PhysAddr::new(0x10_C000));

    // 占满父容量第一个窗口（0x10_0000..0x10_C000）。
    let whole = bank
        .allocate_owned(parent, CapacityKind::User, 0xC000)
        .expect("占满第一个窗口");
    assert_eq!(whole.start(), PhysAddr::new(0x10_0000));

    // second 与父窗口隔着 first 的区间，并回后成为父容量的第二个窗口。
    bank.merge_owned(parent, second, second_range)
        .expect("并回不相邻子窗口");

    // 第一个窗口已无空闲尾部；切分应落到第二个窗口而不是误报失败。
    let child = CapacityId::next();
    let carved = bank
        .split_owned(parent, child, 0x2000)
        .expect("从第二个窗口切出");
    assert_eq!(carved.start(), PhysAddr::new(0x10_E000));
    assert_eq!(bank.used_owned(child), 0);
    assert_eq!(bank.used_owned(parent), 0xC000);
}

#[test]
fn reserve_remaining_claims_unowned_but_not_reserved_windows() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_8000);
    let dma = CapacityId::next();
    let system = CapacityId::next();
    bank.reserve_for(dma, 0x2000).expect("预留 DMA 窗口");

    let span = bank.reserve_remaining(system).expect("登记剩余窗口");
    assert_eq!(span.start(), PhysAddr::new(0x10_2000));
    assert_eq!(span.end(), PhysAddr::new(0x10_8000));

    assert_eq!(
        bank.allocate_owned(dma, CapacityKind::Dma, PAGE_SIZE as u64)
            .expect("DMA 页")
            .start(),
        PhysAddr::new(0x10_0000)
    );
    assert_eq!(
        bank.allocate_owned(system, CapacityKind::System, PAGE_SIZE as u64)
            .expect("System 页")
            .start(),
        PhysAddr::new(0x10_2000)
    );
}

#[test]
fn reserve_remaining_returns_none_when_all_free_is_owned() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_2000);
    bank.reserve_for(CapacityId::next(), 0x2000)
        .expect("占满全部空闲");
    assert!(bank.reserve_remaining(CapacityId::next()).is_none());
}

#[test]
fn kernel_owner_attributes_kernel_allocations() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let system = CapacityId::next();
    bank.reserve_remaining(system).expect("登记 System 窗口");
    bank.set_kernel_owner(Some(system));

    let page = bank.allocate_for(FrameKind::PageTable).expect("页表页");
    assert_eq!(page, PhysAddr::new(0x10_0000));
    assert_eq!(bank.used_owned(system), PAGE_SIZE as u64);

    bank.free_for(FrameKind::PageTable, PhysRange::new(page, PAGE_SIZE as u64));
    assert_eq!(bank.used_owned(system), 0);
}

#[test]
fn adopt_owner_attributes_existing_allocation() {
    let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
    let heap = bank
        .allocate_contiguous_for(FrameKind::KernelHeap, 0x2000)
        .expect("内核堆");
    assert_eq!(heap.start(), PhysAddr::new(0x10_0000));

    let system = CapacityId::next();
    bank.reserve_remaining(system).expect("登记 System 窗口");
    assert!(bank.adopt_owner(system, heap));
    assert_eq!(bank.used_owned(system), 0x2000);

    // 记录不一致时不改归属。
    assert!(!bank.adopt_owner(system, PhysRange::new(heap.start(), 0x1000)));
    assert_eq!(bank.used_owned(system), 0x2000);
}
