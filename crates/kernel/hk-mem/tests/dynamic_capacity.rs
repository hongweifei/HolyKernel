//! User Capacity 的授予配额是硬上限；扩窗只消除碎片。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, RegionKind};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind, PhysicalBackend, set_physical_backend};

static BANK: FrameBank = FrameBank::empty();

#[test]
fn user_capacity_cannot_exceed_grant() {
    BANK.init(
        &[MemoryRegion::new(
            PhysAddr::new(0x10_0000),
            0x10_0000,
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
        .split(user_id, PAGE_SIZE, CapacityKind::User)
        .expect("User 初始窗口");
    assert_eq!(user.limit(), PAGE_SIZE as u64);
    let system_limit_after_split = system.limit();

    assert_eq!(
        user.allocate(PAGE_SIZE * 4).err(),
        Some(hk_mem::hmp::HmpError::OutOfCapacity),
        "子容量不得越过授予配额"
    );
    assert_eq!(user.limit(), PAGE_SIZE as u64);
    assert_eq!(
        system.limit(),
        system_limit_after_split,
        "拒绝分配不得改变父配额"
    );
    assert_eq!(BANK.used_owned(user_id), 0);
}
