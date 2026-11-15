//! NUMA 节点容量与分配的集成测试。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange, RegionKind};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind, set_physical_backend};

static BANK: FrameBank = FrameBank::empty();

fn setup() -> hk_mem::FrameBankStats {
    BANK.init_with_nodes(
        &[MemoryRegion::new(
            PhysAddr::new(0x20_0000),
            0x40_0000,
            RegionKind::Usable,
        )],
        MemoryRegion::new(PhysAddr::new(0x10_0000), 0x1000, RegionKind::Kernel),
        &[
            (PhysRange::new(PhysAddr::new(0x20_0000), 0x20_0000), 0),
            (PhysRange::new(PhysAddr::new(0x40_0000), 0x20_0000), 1),
        ],
    );
    set_physical_backend(&BANK);
    BANK.stats()
}

#[test]
fn splits_and_allocates_on_a_node() {
    let stats = setup();
    assert_eq!(stats.total, 0x40_0000);

    let system_id = CapacityId::next();
    let range = BANK.reserve_remaining(system_id).expect("System 窗口");
    let mut system = Capacity::with_limit(
        system_id,
        range,
        PAGE_SIZE,
        CapacityKind::System,
        BANK.total(),
    )
    .expect("System Capacity");

    let child_id = CapacityId::next();
    let mut node1 = system
        .split_with_page_size_and_node(
            child_id,
            0x10_0000,
            CapacityKind::Numa(1),
            PAGE_SIZE,
            Some(1),
        )
        .expect("节点 1 容量");
    assert_eq!(node1.node(), Some(1));
    let frame = node1.allocate(PAGE_SIZE).expect("节点 1 Frame");
    assert!(
        frame.phys().get() >= 0x40_0000,
        "节点 1 的分配不能落到节点 0"
    );

    let mut node0 = system
        .split_with_page_size_and_node(
            CapacityId::next(),
            0x10_0000,
            CapacityKind::Numa(0),
            PAGE_SIZE,
            Some(0),
        )
        .expect("节点 0 容量");
    let frame0 = node0.allocate(PAGE_SIZE).expect("节点 0 Frame");
    assert!(frame0.phys().get() < 0x40_0000);

    // 绑定节点 1 的容量拒绝节点 0 的偏好，不静默跨节点。
    assert_eq!(
        node1.allocate_on(PAGE_SIZE, Some(0)).err(),
        Some(hk_mem::hmp::HmpError::OutOfCapacity)
    );

    let stats = BANK.stats();
    assert_eq!(stats.by_node[0], PAGE_SIZE as u64);
    assert_eq!(stats.by_node[1], PAGE_SIZE as u64);
}
