//! 子 Capacity 析构时归还窗口与 Ledger。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, RegionKind};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind, set_physical_backend};

static BANK: FrameBank = FrameBank::empty();

#[test]
fn dropping_child_capacity_returns_windows_and_ledger() {
    BANK.init(
        &[MemoryRegion::new(
            PhysAddr::new(0x20_0000),
            0x20_0000,
            RegionKind::Usable,
        )],
        MemoryRegion::new(PhysAddr::new(0), 0, RegionKind::Kernel),
    );
    set_physical_backend(&BANK);

    let system_id = CapacityId::next();
    let range = BANK.reserve_remaining(system_id).expect("System 窗口");
    let mut system = Capacity::with_limit(
        system_id,
        range,
        PAGE_SIZE,
        CapacityKind::System,
        range.len(),
    )
    .expect("System Capacity");
    let user_id = CapacityId::next();
    let mut user = system
        .split(user_id, 0x4000, CapacityKind::User)
        .expect("User Capacity");
    let child_limit = user.limit();
    let parent_limit = system.limit();

    let frame = user.allocate(0x2000).expect("子分配");
    let mut frame = Some(frame);
    user.free(&mut frame).expect("归还子页");
    drop(user);

    assert_eq!(
        system.limit(),
        parent_limit + child_limit,
        "子容量析构必须把配额还给父容量"
    );
    assert!(
        !BANK
            .ledger_stats()
            .iter()
            .any(|entry| entry.capacity == user_id)
    );
    let reused = system.allocate(0x2000).expect("窗口归还后可再次分配");
    assert_eq!(reused.phys(), PhysAddr::new(0x20_0000));
}
