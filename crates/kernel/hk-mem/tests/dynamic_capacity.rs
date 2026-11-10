//! 动态 User Capacity：子容量在配额不足时从父 System 扩窗。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, RegionKind};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind, PhysicalBackend, set_physical_backend};

static BANK: FrameBank = FrameBank::empty();

#[test]
fn user_capacity_grows_from_system_on_demand() {
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

    let frame = user.allocate(PAGE_SIZE * 4).expect("动态扩窗后分配");
    assert_eq!(frame.len(), PAGE_SIZE * 4);
    assert!(
        user.limit() >= (PAGE_SIZE * 5) as u64,
        "User Capacity 应随分配增长"
    );
    assert_eq!(
        BANK.used_owned(user_id),
        (PAGE_SIZE * 4) as u64,
        "扩窗后的页归入 User 账本"
    );

    let mut frame = Some(frame);
    user.free(&mut frame).expect("归还页面");
    assert_eq!(BANK.used_owned(user_id), 0);
}
