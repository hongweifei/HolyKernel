//! Capacity 分配、切分、合并与配额测试。

#![allow(clippy::wildcard_imports)]

use super::*;

fn capacity_with_id(id: u64) -> Capacity {
    Capacity::new(
        CapacityId::new(id),
        PhysRange::new(PhysAddr::new(0x10_0000), 0x10_000),
        4096,
        CapacityKind::User,
        0,
    )
    .expect("Capacity")
}

fn capacity() -> Capacity {
    capacity_with_id(1)
}

#[test]
fn allocates_and_reuses_frames() {
    let mut capacity = capacity();
    let first = capacity.allocate(4096).expect("第一页");
    assert_eq!(first.phys(), PhysAddr::new(0x10_0000));
    assert_eq!(first.len(), 4096);
    assert_eq!(capacity.used(), 4096);

    let second = capacity.allocate(8192).expect("第二段");
    assert_eq!(second.phys(), PhysAddr::new(0x10_1000));
    assert_eq!(second.len(), 8192);

    let mut first = Some(first);
    capacity.free(&mut first).expect("归还");
    assert!(first.is_none());
    assert_eq!(capacity.used(), 8192);
    let reused = capacity.allocate(4096).expect("复用");
    assert_eq!(reused.phys(), PhysAddr::new(0x10_0000));
}

#[test]
fn allocates_large_pages_with_alignment() {
    let mut capacity = Capacity::new(
        CapacityId::new(9),
        PhysRange::new(PhysAddr::new(0x20_1000), 0x40_0000),
        0x20_0000,
        CapacityKind::User,
        0,
    )
    .expect("2 MiB Capacity");
    let frame = capacity.allocate(0x20_0000).expect("2 MiB Frame");
    assert_eq!(frame.page_size(), 0x20_0000);
    assert_eq!(frame.order(), 21);
    assert_eq!(frame.phys(), PhysAddr::new(0x40_0000), "必须对齐到 2 MiB");
}

#[test]
fn split_can_change_page_size() {
    let mut parent = Capacity::new(
        CapacityId::new(6),
        PhysRange::new(PhysAddr::new(0x10_0000), 0x60_0000),
        4096,
        CapacityKind::User,
        0,
    )
    .expect("父容量");
    let mut child = parent
        .split_with_page_size(CapacityId::new(7), 0x40_0000, CapacityKind::User, 0x20_0000)
        .expect("大页子容量");
    assert_eq!(child.page_size(), 0x20_0000);
    let mut frame = Some(child.allocate(0x20_0000).expect("大页 Frame"));
    assert_eq!(frame.as_ref().unwrap().phys().get() % 0x20_0000, 0);
    child.free(&mut frame).expect("归还大页");
    parent.merge(&mut child).expect("并回父容量");
}

#[test]
fn rejects_invalid_split_page_size() {
    let mut parent = capacity();
    assert_eq!(
        parent
            .split_with_page_size(CapacityId::new(8), 0x4000, CapacityKind::User, 0x3000)
            .err(),
        Some(HmpError::InvalidSize)
    );
}

#[test]
fn enforces_limit_and_page_size() {
    let mut capacity = Capacity::new(
        CapacityId::new(2),
        PhysRange::new(PhysAddr::new(0x20_0000), 0x3000),
        4096,
        CapacityKind::System,
        0,
    )
    .expect("Capacity");
    assert_eq!(
        capacity.allocate(0x4000).err(),
        Some(HmpError::OutOfCapacity)
    );
    assert_eq!(capacity.allocate(1).expect("对齐").len(), 4096);
}

#[test]
fn rejects_foreign_and_pinned_frames() {
    let mut first = capacity();
    let mut second = capacity_with_id(2);
    let frame = first.allocate(4096).expect("页");
    let mut frame = Some(frame);
    assert_eq!(
        second.free(&mut frame).err(),
        Some(HmpError::WrongCapability)
    );
    assert!(frame.is_some(), "拒绝归还时 Frame 不能丢失");

    let mut frame = first.allocate(4096).expect("页");
    frame.pin();
    let mut frame = Some(frame);
    assert_eq!(
        first.free(&mut frame).err(),
        Some(HmpError::PermissionDenied)
    );
    assert!(frame.is_some(), "pinned Frame 不能丢失");
}

#[test]
fn frees_frame_once_and_reuses_the_range() {
    let mut capacity = capacity();
    let frame = capacity.allocate(4096).expect("页");
    let mut frame = Some(frame);
    capacity.free(&mut frame).expect("归还");
    assert!(frame.is_none());
    assert_eq!(capacity.used(), 0);
    let reused = capacity.allocate(4096).expect("复用");
    assert_eq!(reused.phys(), PhysAddr::new(0x10_0000));
}

#[test]
fn splits_tail_into_child_capacity() {
    let mut parent = capacity();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    assert_eq!(
        child.range(),
        PhysRange::new(PhysAddr::new(0x10_0000), 0x4000)
    );
    assert_eq!(child.limit(), 0x4000);

    let child_frame = child.allocate(4096).expect("子分配");
    assert_eq!(child_frame.phys(), PhysAddr::new(0x10_0000));
    let parent_frame = parent.allocate(4096).expect("父分配");
    assert_eq!(parent_frame.phys(), PhysAddr::new(0x10_4000));
    assert_eq!(
        parent
            .split(CapacityId::new(3), 0x1_0000, CapacityKind::User)
            .err(),
        Some(HmpError::OutOfCapacity)
    );
}

#[test]
fn tracks_peak_and_soft_limit() {
    let mut capacity = capacity();
    capacity.set_soft_limit(0x8000);
    assert_eq!(capacity.soft_limit(), 0x8000);

    let first = capacity.allocate(0x1000).expect("第一页");
    assert_eq!(capacity.peak(), 0x1000);
    let second = capacity.allocate(0x2000).expect("第二段");
    assert_eq!(capacity.peak(), 0x3000);

    let mut first = Some(first);
    let mut second = Some(second);
    capacity.free(&mut first).expect("归还第一页");
    capacity.free(&mut second).expect("归还第二段");
    assert_eq!(capacity.used(), 0);
    assert_eq!(capacity.peak(), 0x3000, "峰值不随释放回落");
    assert_eq!(capacity.pressure(), 0);
}

#[test]
fn split_window_does_not_charge_parent_usage() {
    let mut parent = capacity();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    // 切出窗口只是把可用区间划走，`used` 只统计已分配的 Frame。
    assert_eq!(parent.used(), 0);

    let frame = child.allocate(4096).expect("子分配");
    assert_eq!(child.used(), 4096);
    assert_eq!(parent.used(), 0);
    let mut frame = Some(frame);
    child.free(&mut frame).expect("归还子页");
    assert_eq!(child.used(), 0);
    assert_eq!(parent.used(), 0);
}

#[test]
fn with_limit_keeps_quota_beyond_single_range() {
    let capacity = Capacity::with_limit(
        CapacityId::new(9),
        PhysRange::new(PhysAddr::new(0x10_0000), 0x2000),
        4096,
        CapacityKind::System,
        0x10_000,
    )
    .expect("System Capacity");
    // 外接范围只描述窗口位置，配额按全部窗口总量声明。
    assert_eq!(capacity.range().len(), 0x2000);
    assert_eq!(capacity.limit(), 0x10_000);
}

#[test]
fn merges_empty_child_back_into_tail() {
    let mut parent = capacity();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    assert_eq!(parent.used(), 0);

    parent.merge(&mut child).expect("并回尾部");
    assert_eq!(parent.used(), 0);
    let frame = parent.allocate(0x4000).expect("并回后分配");
    assert_eq!(frame.phys(), PhysAddr::new(0x10_0000));
}

#[test]
fn split_reduces_parent_limit_and_merge_restores() {
    let mut parent = capacity();
    let original = parent.limit();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    assert_eq!(parent.limit(), original - 0x4000);
    parent.merge(&mut child).expect("并回");
    assert_eq!(parent.limit(), original);
}

#[test]
fn merge_rejects_unrelated_empty_capacity() {
    let mut first = capacity();
    let mut second = capacity_with_id(2);
    assert_eq!(
        first.merge(&mut second).err(),
        Some(HmpError::WrongCapability)
    );
}

#[test]
fn merge_rejects_child_with_live_frames() {
    let mut parent = capacity();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    let _frame = child.allocate(4096).expect("子分配");
    assert_eq!(
        parent.merge(&mut child).err(),
        Some(HmpError::PermissionDenied)
    );
}

#[test]
fn merge_accepts_child_after_frames_return() {
    let mut parent = capacity();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    let frame = child.allocate(4096).expect("子分配");
    let mut frame = Some(frame);
    child.free(&mut frame).expect("归还子页");
    parent.merge(&mut child).expect("并回尾部");

    // 子容量切走的区间回到父容量，父容量可以再次切出同等子容量。
    let again = parent
        .split(CapacityId::new(3), 0x4000, CapacityKind::User)
        .expect("再次切出");
    assert_eq!(
        again.range(),
        PhysRange::new(PhysAddr::new(0x10_0000), 0x4000)
    );
}

#[test]
fn release_rejects_unowned_and_repeated_ranges() {
    let mut parent = capacity();
    let frame = parent.allocate(4096).expect("分配");
    let mut frame = Some(frame);
    parent.free(&mut frame).expect("归还");
    assert_eq!(
        parent.release(PhysAddr::new(0x10_0000), 4096).err(),
        Some(HmpError::NotMapped),
        "重复释放必须拒绝"
    );
    assert_eq!(
        parent.release(PhysAddr::new(0x10_3000), 4096).err(),
        Some(HmpError::NotMapped),
        "未分配区间必须拒绝"
    );
}

#[test]
fn merge_rejects_repeated_merge() {
    let mut parent = capacity();
    let original = parent.limit();
    let mut child = parent
        .split(CapacityId::new(2), 0x4000, CapacityKind::User)
        .expect("子 Capacity");
    parent.merge(&mut child).expect("并回");
    assert_eq!(parent.limit(), original);
    assert_eq!(
        parent.merge(&mut child).err(),
        Some(HmpError::WrongCapability),
        "已并回的容量不能再次 merge"
    );
    assert_eq!(parent.limit(), original, "配额不得重复放大");
}

#[test]
fn merge_defers_grandchild_windows_until_they_drop() {
    let mut parent = capacity();
    let original = parent.limit();
    let mut child = parent
        .split(CapacityId::new(2), 0x8000, CapacityKind::User)
        .expect("子 Capacity");
    let mut grandchild = child
        .split(CapacityId::new(3), 0x4000, CapacityKind::User)
        .expect("孙 Capacity");

    // 孙窗口仍在：配额立即并回，但窗口要等孙容量析构才归还。
    let child_range = child.range();
    parent.merge(&mut child).expect("孙窗口存在时仍可并回");
    drop(child);
    let reserved = parent.allocate(0x4000).expect("父分配剩余窗口");
    assert!(reserved.phys().get() >= child_range.start().get());

    let frame = grandchild.allocate(4096).expect("孙容量仍可分配");
    assert!(frame.phys().get() >= child_range.start().get());
    let mut frame = Some(frame);
    grandchild.free(&mut frame).expect("归还孙页");

    drop(grandchild);
    assert_eq!(parent.limit(), original, "孙容量析构后配额必须回到父容量");
    let reclaimed = parent.allocate(0x4000).expect("孙窗口归还后可分配");
    assert!(reclaimed.phys().get() >= child_range.start().get());
}
