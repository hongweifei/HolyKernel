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
        .split(user_id, 0x4_0000, CapacityKind::User)
        .expect("User Capacity");

    // 让窗口内只剩不连续的 64 KiB 空闲段，再申请 192 KiB，触发一次扩窗。
    let mut first = Some(user.allocate(0x1_0000).expect("窗口 A"));
    let mut second = Some(user.allocate(0x1_0000).expect("窗口 B"));
    user.free(&mut first).expect("归还窗口 A");
    let frame = user.allocate(0x3_0000).expect("动态扩窗后分配");
    assert!(
        user.windows()
            .iter()
            .map(|window| window.len())
            .sum::<u64>()
            > 0x4_0000,
        "grow 应扩出额外窗口"
    );
    let grown = user.windows();
    let mut frame = Some(frame);
    user.free(&mut frame).expect("归还子页");
    user.free(&mut second).expect("归还窗口 B");

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

    // 孙容量在中间容量已 merge 后析构：窗口必须沿 split 记录挂回仍存活的
    // System 容量，而不是已销毁的中间 CapacityId。
    let middle_id = CapacityId::next();
    let mut middle = system
        .split(middle_id, 0x2_0000, CapacityKind::User)
        .expect("中间容量");
    let grandchild_id = CapacityId::next();
    let mut grandchild = middle
        .split(grandchild_id, 0x1_0000, CapacityKind::User)
        .expect("孙容量");
    let grand_range = grandchild.windows()[0];
    let mut frame = Some(grandchild.allocate(PAGE_SIZE).expect("孙容量分配"));
    grandchild.free(&mut frame).expect("归还孙容量页");
    system.merge(&mut middle).expect("先并回中间容量");
    drop(middle);
    drop(grandchild);
    assert!(
        system.windows().iter().any(|window| {
            window.start().get() <= grand_range.start().get()
                && window.end().get() >= grand_range.end().get()
        }),
        "孙窗口必须沿代际挂回 System，而不是死亡 CapacityId"
    );
    system.allocate(0x2000).expect("孙窗口可再次分配");

    // 析构时仍有未归还的裸 Frame：强制回收也必须归还窗口并移除 Ledger，
    // 不能把窗口永久挂在已销毁的 CapacityId 上。
    let leak_id = CapacityId::next();
    let mut leak = system
        .split(leak_id, 0x1_0000, CapacityKind::User)
        .expect("临时容量");
    let _raw = leak.allocate(PAGE_SIZE).expect("裸 Frame");
    drop(leak);
    assert!(
        !BANK
            .ledger_stats()
            .iter()
            .any(|entry| entry.capacity == leak_id),
        "析构后不得保留死亡 CapacityId 的 Ledger"
    );
    assert!(
        system
            .windows()
            .iter()
            .any(|window| window.len() >= 0x1_0000),
        "强制回收应把窗口还给父容量"
    );
}
