//! 窗口碎片化时的容量分配：配额尚有余量也必须能继续分配。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, RegionKind};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind, set_physical_backend};

static BANK: FrameBank = FrameBank::empty();

const REGION_START: u64 = 0x20_0000;
const REGION_LEN: u64 = 0x400_0000;

fn reset() -> (Capacity, Capacity) {
    BANK.init(
        &[MemoryRegion::new(
            PhysAddr::new(REGION_START),
            REGION_LEN,
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
    let user = system
        .split(user_id, PAGE_SIZE, CapacityKind::User)
        .expect("User Capacity");
    (system, user)
}

#[test]
fn fragmentation_does_not_report_out_of_capacity() {
    // 场景一：三块等长窗口中间那块仍在使用，两端空闲但被已用窗口隔开。
    // 配额仍有富余，任何窗口都放不下新请求时必须扩出新窗口，而不是返回
    // OutOfCapacity。
    let (_system, mut user) = reset();
    let unit = 0x10_0000_u64; // 1 MiB
    let mut first = Some(user.allocate(unit as usize).expect("窗口 A"));
    let mut middle = Some(user.allocate(unit as usize).expect("窗口 B"));
    let mut last = Some(user.allocate(unit as usize).expect("窗口 C"));
    user.free(&mut first).expect("归还窗口 A");
    user.free(&mut last).expect("归还窗口 C");

    let windows = user.windows().len();
    let mut grown = Some(
        user.allocate((unit * 2) as usize)
            .expect("碎片化不能伪装成容量耗尽"),
    );
    assert_eq!(
        user.windows().len(),
        windows + 1,
        "扩出的连续窗口必须登记在容量名下"
    );
    user.free(&mut grown).expect("归还扩窗分配");
    user.free(&mut middle).expect("归还窗口 B");

    // 场景二：两个相邻窗口各自空闲段都短于请求，但合并后连续足够。
    let (_system, mut user) = reset();
    let first = 0x10_0000_u64;
    let second = 0x20_0000_u64;
    let request = first + second;

    let mut a = Some(user.allocate(first as usize).expect("窗口 A"));
    let mut b = Some(user.allocate(second as usize).expect("窗口 B"));
    user.free(&mut a).expect("归还窗口 A");
    user.free(&mut b).expect("归还窗口 B");

    let windows = user.windows().len();
    let mut merged = Some(
        user.allocate(request as usize)
            .expect("相邻空闲段合并后必须可分配"),
    );
    assert_eq!(
        user.windows().len(),
        windows,
        "相邻窗口的空闲段合并后不需要再扩窗"
    );
    user.free(&mut merged).expect("归还合并分配");
}
