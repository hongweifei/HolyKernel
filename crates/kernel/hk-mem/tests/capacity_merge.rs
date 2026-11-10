//! 显式 merge 归还子容量的全部窗口。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, RegionKind};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind, PhysicalBackend, set_physical_backend};

static BANK: FrameBank = FrameBank::empty();

#[test]
fn merge_returns_grown_windows_to_parent() {
    BANK.init(
        &[MemoryRegion::new(
            PhysAddr::new(0x30_0000),
            0x40_0000,
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

    let frame = user.allocate(0x1_0000).expect("动态扩窗后分配");
    assert_eq!(user.windows().len(), 2, "grow 应产生第二个窗口");
    let grown = user.windows();
    let mut frame = Some(frame);
    user.free(&mut frame).expect("归还子页");

    system.merge(&mut user).expect("并回全部窗口");
    assert_eq!(system.limit(), range.len(), "父配额应在并回后完整恢复");
    assert_eq!(BANK.used_owned(user_id), 0);
    assert!(
        !BANK
            .ledger_stats()
            .iter()
            .any(|entry| entry.capacity == user_id)
    );

    let parent_windows = system.windows();
    for window in grown {
        assert!(
            parent_windows.iter().any(|parent| {
                parent.start().get() <= window.start().get()
                    && parent.end().get() >= window.end().get()
            }),
            "增长的窗口必须回到父容量"
        );
    }
    drop(user);
    system.allocate(0x2000).expect("并回后的窗口可再次分配");
}
