//! 内核集成测试。

use hk_device::{DeviceGrant, MmioRange, PciAddress};
use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
use hk_handle::{HandleTable, ObjectKind, Rights};
use hk_ipc::Message;
use hk_mem::hmp::{
    Capability, CapabilityKind, Capacity, CapacityId, CapacityKind, FrameBacking, FrameLease,
    ProviderKind, Region, RegionFlags,
};

/// Provider 释放通知标签（与 ABI 一致）。
const PROVIDER_RELEASE: u64 = 0x1205;

use crate::{Kernel, TcbId, ThreadState, TraceCategory, take_provider_releases};
use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, DEFAULT_SPEC, millis};

use core::sync::atomic::{AtomicUsize, Ordering};

mod cadence;
mod scheduling;

static RESCHED_CALLS: AtomicUsize = AtomicUsize::new(0);

fn count_resched(_cpu: usize) {
    RESCHED_CALLS.fetch_add(1, Ordering::SeqCst);
}

fn kernel() -> Kernel {
    Kernel::new()
}

fn thread(kernel: &Kernel) -> TcbId {
    kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务")
}

#[test]
fn creates_objects_and_grants_handles() {
    let kernel = kernel();
    let worker = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(worker, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let notification = kernel
        .create_notification(worker, Rights::READ | Rights::WRITE)
        .expect("创建通知");

    assert_eq!(kernel.live_objects(), 2);
    assert_eq!(kernel.handles_live(worker), Some(2));
    assert_ne!(
        kernel.handle_id(worker, endpoint).expect("端点句柄"),
        kernel.handle_id(worker, notification).expect("通知句柄")
    );
}

#[test]
fn creates_hmp_objects() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;

    let mut capacity = Capacity::new(
        CapacityId::new(1),
        PhysRange::new(PhysAddr::new(0x10_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::System,
        0,
    )
    .expect("Capacity");
    let frame = capacity.allocate(PAGE_SIZE).expect("Frame");
    let capability =
        Capability::new(CapabilityKind::Anonymous, 0x2000, PAGE_SIZE).expect("Capability");
    let region =
        Region::new(&capability, 0, 0x1000, Rights::READ, RegionFlags::empty()).expect("Region");
    kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    kernel
        .create_frame(worker, frame, rights)
        .expect("创建 Frame");
    kernel
        .create_capability(worker, capability, None, rights)
        .expect("创建 Capability");
    kernel
        .create_region(worker, region, rights)
        .expect("创建 Region");

    assert_eq!(kernel.live_objects(), 4);
    assert_eq!(kernel.handles_live(worker), Some(4));
}

#[test]
fn allocates_splits_and_frees_capacity_frames() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER;
    let capacity = Capacity::new(
        CapacityId::new(10),
        PhysRange::new(PhysAddr::new(0x30_0000), 0x20_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");

    let frame = kernel
        .allocate_frame(worker, capacity, PAGE_SIZE, None)
        .expect("分配 Frame");
    let (range, page_size, _, refs, pinned) = kernel.frame_info(worker, frame).expect("Frame 信息");
    assert_eq!(range.start(), PhysAddr::new(0x30_0000));
    assert_eq!(page_size, PAGE_SIZE);
    assert_eq!(refs, 1);
    assert!(!pinned);
    assert_eq!(kernel.frame_resident(worker, frame), Ok(PAGE_SIZE as u64));
    assert_eq!(kernel.pin_frame(worker, frame, true), Ok(()));
    assert_eq!(
        kernel.free_frame(worker, capacity, frame).err(),
        Some(crate::KernelError::Hmp(
            hk_mem::hmp::HmpError::PermissionDenied
        ))
    );
    assert_eq!(kernel.pin_frame(worker, frame, false), Ok(()));
    assert_eq!(kernel.free_frame(worker, capacity, frame), Ok(()));
    assert_eq!(kernel.frame_resident(worker, frame), Ok(0));
    assert!(matches!(
        kernel.frame_info(worker, frame),
        Err(crate::KernelError::Hmp(hk_mem::hmp::HmpError::NotMapped))
    ));

    let child = kernel
        .split_capacity(worker, capacity, 0x4000, CapacityKind::User, 0, None)
        .expect("分割 Capacity");
    let (child_range, kind, _, limit, used) = kernel
        .capacity_info(worker, child)
        .expect("子 Capacity 信息");
    assert_eq!(child_range.start(), PhysAddr::new(0x30_1000));
    assert_eq!(kind, CapacityKind::User);
    assert_eq!(limit, 0x4000);
    assert_eq!(used, 0);
}

#[test]
fn pinned_frame_is_not_returned_to_capacity() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER;
    let capacity = Capacity::new(
        CapacityId::new(12),
        PhysRange::new(PhysAddr::new(0x50_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    let frame = kernel
        .allocate_frame(worker, capacity, PAGE_SIZE, None)
        .expect("分配 Frame");
    kernel.pin_frame(worker, frame, true).expect("pin");
    assert_eq!(
        kernel.free_frame(worker, capacity, frame).err(),
        Some(crate::KernelError::Hmp(
            hk_mem::hmp::HmpError::PermissionDenied
        )),
        "固定驻留的 Frame 不能被归还"
    );
    let (_, _, _, refs, pinned) = kernel.frame_info(worker, frame).expect("Frame 仍在");
    assert_eq!(refs, 1, "拒绝归还不能改变句柄数");
    assert!(pinned);

    kernel.pin_frame(worker, frame, false).expect("unpin");
    kernel.free_frame(worker, capacity, frame).expect("归还");
    assert_eq!(
        kernel.capacity_info(worker, capacity).expect("Capacity").4,
        0,
        "释放后物理区间归还容量"
    );
    assert!(matches!(
        kernel.frame_info(worker, frame),
        Err(crate::KernelError::Hmp(hk_mem::hmp::HmpError::NotMapped))
    ));
}

#[test]
fn dropping_pinned_frame_handle_returns_it_to_capacity() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER;
    let capacity = Capacity::new(
        CapacityId::new(13),
        PhysRange::new(PhysAddr::new(0x60_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    let frame = kernel
        .allocate_frame(worker, capacity, PAGE_SIZE, None)
        .expect("分配 Frame");
    kernel.pin_frame(worker, frame, true).expect("pin");
    kernel.close_handle(worker, frame).expect("关闭 Frame");
    assert_eq!(
        kernel.capacity_info(worker, capacity).expect("Capacity").4,
        0,
        "最后一个 pinned 句柄销毁后必须归还物理页"
    );
}

#[test]
fn taking_pinned_frame_clears_pin_and_returns_page() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER;
    let capacity = Capacity::new(
        CapacityId::new(14),
        PhysRange::new(PhysAddr::new(0x70_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    let frame = kernel
        .allocate_frame(worker, capacity, PAGE_SIZE, None)
        .expect("分配 Frame");
    kernel.pin_frame(worker, frame, true).expect("pin");

    // `SYS_FRAME_MAP` 走这条路径：pin 不随所有权转移，否则归还时会被拒绝。
    let owned = kernel.take_owned_frame(worker, frame).expect("取走 Frame");
    assert_eq!(owned.len(), PAGE_SIZE);
    drop(owned);
    assert_eq!(
        kernel.capacity_info(worker, capacity).expect("Capacity").4,
        0,
        "转移出的 pinned Frame 销毁时必须归还物理页"
    );
}

#[test]
fn closing_last_frame_handle_returns_it_to_capacity() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER;
    let capacity = Capacity::new(
        CapacityId::new(11),
        PhysRange::new(PhysAddr::new(0x40_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    let frame = kernel
        .allocate_frame(worker, capacity, PAGE_SIZE, None)
        .expect("分配 Frame");
    assert_eq!(
        kernel
            .capacity_info(worker, capacity)
            .expect("Capacity 信息")
            .4,
        PAGE_SIZE as u64
    );
    assert_eq!(
        kernel.capacity_stats(worker, capacity),
        Ok((PAGE_SIZE as u64, 0)),
        "按容量记录峰值"
    );

    kernel.close_handle(worker, frame).expect("关闭 Frame");
    assert_eq!(
        kernel
            .capacity_info(worker, capacity)
            .expect("Capacity 信息")
            .4,
        0
    );
}

#[test]
fn reports_capability_resident_bytes() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let capability =
        Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE).expect("能力");
    let backing = FrameBacking::Imported(FrameLease::import(PhysAddr::new(0x70_0000), PAGE_SIZE));
    let handle = kernel
        .create_capability(
            worker,
            capability,
            Some(backing),
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )
        .expect("创建能力");
    assert_eq!(
        kernel.capability_resident(worker, handle),
        Ok(PAGE_SIZE as u64)
    );
}

#[test]
fn capability_release_notifies_provider() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let endpoint = kernel
        .create_endpoint(worker, Rights::READ | Rights::WRITE)
        .expect("端点");
    let endpoint_id = kernel.handle_id(worker, endpoint).expect("端点标识");
    let capability =
        Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE).expect("能力");
    let handle = kernel
        .create_capability(
            worker,
            capability,
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )
        .expect("能力句柄");
    let capability_id = kernel.handle_id(worker, handle).expect("能力标识");
    kernel
        .bind_capability_provider(worker, handle, endpoint, ProviderKind::Anonymous)
        .expect("绑定 Provider");
    kernel.close_handle(worker, handle).expect("关闭能力");

    let releases = take_provider_releases();
    assert!(releases.contains(&(capability_id, endpoint_id)));

    kernel
        .post_to_endpoint(
            endpoint_id,
            Message::new(PROVIDER_RELEASE).with_words([capability_id.get(), 0, 0, 0, 0, 0, 0, 0]),
        )
        .expect("投递释放通知");
    let received = kernel
        .recv_from(worker, endpoint, None)
        .expect("接收")
        .expect("通知消息");
    assert_eq!(received.message.label, PROVIDER_RELEASE);
    assert_eq!(received.message.words[0], capability_id.get());
}

#[test]
fn object_ledger_reads_capability_and_frame() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;

    let capability =
        Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE).expect("能力");
    let capability_handle = kernel
        .create_capability(worker, capability, None, rights)
        .expect("能力句柄");
    assert_eq!(
        kernel
            .object_ledger(worker, capability_handle)
            .expect("能力 Ledger")
            .charged,
        0
    );
    let page = FrameBacking::Imported(FrameLease::import(PhysAddr::new(0x70_0000), PAGE_SIZE));
    assert_eq!(
        kernel.set_capability_page(worker, capability_handle, 0, page),
        Ok(true)
    );
    assert_eq!(
        kernel
            .object_ledger(worker, capability_handle)
            .expect("能力 Ledger")
            .charged,
        PAGE_SIZE as u64
    );

    let capacity = Capacity::new(
        CapacityId::new(2),
        PhysRange::new(PhysAddr::new(0x30_0000), 0x4000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = kernel
        .create_capacity(worker, capacity, rights)
        .expect("Capacity 句柄");
    let frame_handle = kernel
        .allocate_frame(worker, capacity_handle, PAGE_SIZE, None)
        .expect("Frame 句柄");
    assert_eq!(
        kernel
            .object_ledger(worker, frame_handle)
            .expect("Frame Ledger")
            .charged,
        PAGE_SIZE as u64
    );
}

#[test]
fn object_ledger_reports_utilization_not_quota() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;

    let capability =
        Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE).expect("能力");
    let mut region = Region::new(
        &capability,
        0,
        PAGE_SIZE,
        Rights::READ | Rights::WRITE,
        RegionFlags::empty(),
    )
    .expect("Region");
    region.map(VirtAddr::new(0x40_0000)).expect("映射");
    let region_handle = kernel
        .create_region(worker, region, rights)
        .expect("Region 句柄");
    let region_ledger = kernel
        .object_ledger(worker, region_handle)
        .expect("Region Ledger");
    assert_eq!(region_ledger.charged, PAGE_SIZE as u64);
    assert_eq!(region_ledger.limit, PAGE_SIZE as u64);
    assert_eq!(region_ledger.peak, PAGE_SIZE as u64);
    assert_eq!(region_ledger.pressure, 100);

    let capability = Capability::new(CapabilityKind::Anonymous, (PAGE_SIZE * 2) as u64, PAGE_SIZE)
        .expect("能力");
    let capability_handle = kernel
        .create_capability(worker, capability, None, rights)
        .expect("能力句柄");
    let page = FrameBacking::Imported(FrameLease::import(PhysAddr::new(0x70_0000), PAGE_SIZE));
    assert_eq!(
        kernel.set_capability_page(worker, capability_handle, 0, page),
        Ok(true)
    );
    let capability_ledger = kernel
        .object_ledger(worker, capability_handle)
        .expect("能力 Ledger");
    assert_eq!(capability_ledger.charged, PAGE_SIZE as u64);
    assert_eq!(capability_ledger.limit, (PAGE_SIZE * 2) as u64);
    assert_eq!(capability_ledger.peak, PAGE_SIZE as u64);
    assert_eq!(capability_ledger.pressure, 50);
}

#[test]
fn capability_ledger_tracks_lazy_pages() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let capability = Capability::new(CapabilityKind::Anonymous, (PAGE_SIZE * 2) as u64, PAGE_SIZE)
        .expect("能力");
    let handle = kernel
        .create_capability(
            worker,
            capability,
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )
        .expect("创建能力");
    assert_eq!(kernel.capability_resident(worker, handle), Ok(0));

    let page = FrameBacking::Imported(FrameLease::import(PhysAddr::new(0x70_0000), PAGE_SIZE));
    assert_eq!(
        kernel.set_capability_page(worker, handle, 0, page),
        Ok(true)
    );
    assert_eq!(
        kernel.capability_resident(worker, handle),
        Ok(PAGE_SIZE as u64)
    );
    assert!(
        kernel
            .take_capability_page(worker, handle, 0)
            .unwrap()
            .is_some()
    );
    assert_eq!(kernel.capability_resident(worker, handle), Ok(0));
}

#[test]
fn last_handle_returns_frame_to_capacity() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER;
    let capacity = Capacity::new(
        CapacityId::new(12),
        PhysRange::new(PhysAddr::new(0x50_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    let frame = kernel
        .allocate_frame(worker, capacity, PAGE_SIZE, None)
        .expect("分配 Frame");

    let copy = kernel
        .duplicate_handle(worker, frame, rights)
        .expect("复制句柄");
    assert_eq!(
        kernel
            .capacity_info(worker, capacity)
            .expect("Capacity 信息")
            .4,
        PAGE_SIZE as u64,
        "仍有句柄时不归还"
    );
    kernel.close_handle(worker, copy).expect("关闭副本");
    assert_eq!(
        kernel.frame_info(worker, frame).expect("Frame 信息").3,
        1,
        "帧引用就是句柄数"
    );

    kernel.free_frame(worker, capacity, frame).expect("归还");
    assert_eq!(
        kernel
            .capacity_info(worker, capacity)
            .expect("Capacity 信息")
            .4,
        0,
        "最后一个句柄归还 Capacity"
    );
    assert!(matches!(
        kernel.frame_info(worker, frame),
        Err(crate::KernelError::Hmp(hk_mem::hmp::HmpError::NotMapped))
    ));
    kernel.close_handle(worker, frame).expect("关闭 Frame");
}

#[test]
fn tracks_frame_refs() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;
    let mut capacity = Capacity::new(
        CapacityId::new(2),
        PhysRange::new(PhysAddr::new(0x20_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let frame = capacity.allocate(PAGE_SIZE).expect("Frame");
    let frame = kernel
        .create_frame(worker, frame, rights)
        .expect("创建 Frame");

    assert_eq!(kernel.frame_info(worker, frame).expect("Frame 信息").3, 1);
    let copy = kernel
        .duplicate_handle(worker, frame, rights)
        .expect("复制句柄");
    assert_eq!(kernel.frame_info(worker, frame).expect("Frame 信息").3, 2);
    kernel.close_handle(worker, copy).expect("关闭副本");
    kernel.pin_frame(worker, frame, true).expect("pin");
    assert!(kernel.frame_range(worker, frame).is_ok());
}

#[test]
fn commits_revision_capability_versions() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let capability =
        Capability::new(CapabilityKind::Revision, 0x1000, PAGE_SIZE).expect("Capability");
    let handle = kernel
        .create_capability(
            worker,
            capability,
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )
        .expect("创建 Revision 能力");
    assert_eq!(kernel.commit_capability(worker, handle), Ok(1));
    assert_eq!(kernel.commit_capability(worker, handle), Ok(2));
}

#[test]
fn traces_handle_grants_and_closes() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let before = kernel.trace_len();

    let endpoint = kernel
        .create_endpoint(worker, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    assert_eq!(kernel.trace_len(), before + 1);

    kernel.close_handle(worker, endpoint).expect("关闭句柄");
    assert_eq!(kernel.trace_len(), before + 2);

    kernel.exit(worker).expect("退出线程");
    assert_eq!(kernel.trace_len(), before + 4);

    let events = kernel.trace_events(4);
    assert_eq!(events[0].category, TraceCategory::Handle);
    assert_eq!(events[0].code, 1);
    assert_eq!(events[1].category, TraceCategory::Handle);
    assert_eq!(events[1].code, 12);
    assert_eq!(events[3].category, TraceCategory::Handle);
    assert_eq!(events[3].code, 14);
}

#[test]
fn send_delivers_to_waiting_receiver() {
    let kernel = kernel();
    let sender = thread(&kernel);
    let receiver = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(sender, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(sender, endpoint).expect("端点标识");
    let receiver_endpoint = kernel
        .grant(receiver, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予接收方句柄");

    assert!(
        kernel
            .recv(receiver, receiver_endpoint)
            .expect("接收")
            .is_none()
    );
    assert_eq!(kernel.state_of(receiver), Some(ThreadState::Blocked));

    kernel
        .send(
            sender,
            endpoint,
            Message::new(11).with_words([7, 0, 0, 0, 0, 0, 0, 0]),
        )
        .expect("发送");

    assert_eq!(kernel.state_of(receiver), Some(ThreadState::Runnable));
    let received = kernel
        .recv(receiver, receiver_endpoint)
        .expect("接收")
        .expect("收到消息");
    assert_eq!(received.message.label, 11);
    assert_eq!(received.message.words[0], 7);
    assert_eq!(received.sender, sender);
}

#[test]
fn cancel_recv_distinguishes_idle_timeout_from_claimed_delivery() {
    let kernel = kernel();
    let sender = thread(&kernel);
    let receiver = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(sender, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(sender, endpoint).expect("端点标识");
    let receiver_endpoint = kernel
        .grant(receiver, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予接收方句柄");

    assert!(
        kernel
            .recv(receiver, receiver_endpoint)
            .expect("接收")
            .is_none()
    );
    assert!(
        kernel
            .cancel_recv(receiver, receiver_endpoint)
            .expect("取消空等待"),
        "没有发送方时取消应报告已移除等待者"
    );

    assert!(
        kernel
            .recv(receiver, receiver_endpoint)
            .expect("接收")
            .is_none()
    );
    kernel
        .send(sender, endpoint, Message::new(11))
        .expect("发送");
    assert!(
        !kernel
            .cancel_recv(receiver, receiver_endpoint)
            .expect("发送方已认领"),
        "发送方认领后取消不应报告移除了等待者"
    );
    let received = kernel
        .recv(receiver, receiver_endpoint)
        .expect("接收")
        .expect("消息不应丢失");
    assert_eq!(received.message.label, 11);
}

#[test]
fn try_recv_does_not_block_receiver() {
    let kernel = kernel();
    let owner = thread(&kernel);
    let receiver = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(owner, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(owner, endpoint).expect("端点标识");
    let handle = kernel
        .grant(receiver, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予接收方句柄");

    assert_eq!(kernel.state_of(receiver), Some(ThreadState::Runnable));
    assert!(
        kernel
            .try_recv_from(receiver, handle, None)
            .expect("非阻塞接收")
            .is_none()
    );
    // 非阻塞接收在没有消息时不得把线程置为阻塞，否则调度器不会再重排它。
    assert_eq!(kernel.state_of(receiver), Some(ThreadState::Runnable));
}

#[test]
fn queues_message_without_blocking_sender() {
    let kernel = kernel();
    let sender = thread(&kernel);
    let receiver = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(sender, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(sender, endpoint).expect("端点标识");
    let receiver_endpoint = kernel
        .grant(receiver, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予接收方句柄");

    kernel
        .send(sender, endpoint, Message::new(3))
        .expect("发送");
    assert_eq!(kernel.state_of(sender), Some(ThreadState::Runnable));
    assert_eq!(kernel.runnable(), 2);

    let received = kernel
        .recv(receiver, receiver_endpoint)
        .expect("接收")
        .expect("收到消息");
    assert_eq!(received.message.label, 3);
    assert_eq!(kernel.state_of(sender), Some(ThreadState::Runnable));
}

#[test]
fn send_reports_queue_full() {
    let kernel = kernel();
    let sender = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(sender, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    for _ in 0..hk_ipc::ENDPOINT_QUEUE_CAPACITY {
        kernel
            .send(sender, endpoint, Message::new(1))
            .expect("入队");
    }
    assert_eq!(
        kernel.send(sender, endpoint, Message::new(2)),
        Err(crate::KernelError::QueueFull)
    );
}

#[test]
fn recv_from_skips_other_senders() {
    let kernel = kernel();
    let first = thread(&kernel);
    let second = thread(&kernel);
    let receiver = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(first, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(first, endpoint).expect("端点标识");
    let receiver_endpoint = kernel
        .grant(receiver, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予接收方句柄");
    let second_endpoint = kernel
        .grant(second, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予第二个发送方句柄");

    kernel
        .send(second, second_endpoint, Message::new(2))
        .expect("发送");
    assert!(
        kernel
            .recv_from(receiver, receiver_endpoint, Some(first))
            .expect("接收")
            .is_none()
    );

    kernel.send(first, endpoint, Message::new(1)).expect("发送");
    let received = kernel
        .recv_from(receiver, receiver_endpoint, Some(first))
        .expect("接收")
        .expect("收到第一个发送方的消息");
    assert_eq!(received.sender, first);
    assert_eq!(received.message.label, 1);

    let received = kernel
        .recv(receiver, receiver_endpoint)
        .expect("接收")
        .expect("收到排队消息");
    assert_eq!(received.sender, second);
    assert_eq!(received.message.label, 2);
}

#[test]
fn call_and_reply_round_trip() {
    let kernel = kernel();
    let client = thread(&kernel);
    let server = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(client, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(client, endpoint).expect("端点标识");
    let server_endpoint = kernel
        .grant(server, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予服务端句柄");

    assert!(
        kernel
            .recv(server, server_endpoint)
            .expect("接收")
            .is_none()
    );

    kernel
        .call(client, endpoint, Message::new(21))
        .expect("调用");
    assert_eq!(kernel.state_of(client), Some(ThreadState::Blocked));

    let received = kernel
        .recv(server, server_endpoint)
        .expect("接收")
        .expect("收到调用");
    assert_eq!(received.message.label, 21);

    let token = received.reply.expect("调用携带回复令牌");
    kernel.reply(server, token, Message::new(22)).expect("回复");

    assert_eq!(kernel.state_of(client), Some(ThreadState::Runnable));
    assert_eq!(kernel.take_reply(client), Some(Message::new(22)));
    assert_eq!(kernel.take_reply(client), None);
}

#[test]
fn server_exit_fails_pending_calls() {
    let kernel = kernel();
    let client = thread(&kernel);
    let server = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("服务端创建端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(client, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予客户端句柄");

    assert!(kernel.recv(server, endpoint).expect("接收").is_none());
    kernel
        .call(client, client_endpoint, Message::new(7))
        .expect("调用");
    assert_eq!(kernel.state_of(client), Some(ThreadState::Blocked));

    kernel.exit(server).expect("服务端退出");
    assert_eq!(
        kernel.state_of(client),
        Some(ThreadState::Runnable),
        "服务端退出应唤醒调用方"
    );
    let reply = kernel.take_reply(client).expect("错误回复");
    assert_eq!(reply.label, u64::MAX);
}

#[test]
fn rejects_invalid_reply_token() {
    let kernel = kernel();
    let client = thread(&kernel);
    let server = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(client, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(client, endpoint).expect("端点标识");
    let server_endpoint = kernel
        .grant(server, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予服务端句柄");

    assert!(
        kernel
            .recv(server, server_endpoint)
            .expect("接收")
            .is_none()
    );
    kernel
        .call(client, endpoint, Message::new(1))
        .expect("调用");
    let received = kernel
        .recv(server, server_endpoint)
        .expect("接收")
        .expect("收到调用");
    let token = received.reply.expect("回复令牌");

    let forged = crate::ReplyToken {
        caller: client,
        sequence: 99,
    };
    assert_eq!(
        kernel.reply(server, forged, Message::new(2)).err(),
        Some(crate::KernelError::InvalidReply)
    );

    assert_eq!(kernel.reply(server, token, Message::new(3)), Ok(()));
    assert_eq!(
        kernel.reply(server, token, Message::new(4)).err(),
        Some(crate::KernelError::InvalidReply)
    );
}

#[test]
fn notification_wakes_waiter() {
    let kernel = kernel();
    let waiter = thread(&kernel);
    let signaler = thread(&kernel);

    let notification = kernel
        .create_notification(waiter, Rights::READ | Rights::WRITE)
        .expect("创建通知");
    let notification_id = kernel.handle_id(waiter, notification).expect("通知标识");
    let signal_handle = kernel
        .grant(signaler, notification_id, Rights::READ | Rights::WRITE)
        .expect("授予句柄");

    assert_eq!(kernel.wait(waiter, notification, 0b1).expect("等待"), None);
    assert_eq!(kernel.state_of(waiter), Some(ThreadState::Blocked));

    kernel.signal(signaler, signal_handle, 0b1).expect("置位");
    assert_eq!(kernel.state_of(waiter), Some(ThreadState::Runnable));
    assert_eq!(
        kernel.wait(waiter, notification, 0b1).expect("等待"),
        Some(0b1)
    );
}

#[test]
fn transfers_handles_over_ipc() {
    let kernel = kernel();
    let client = thread(&kernel);
    let server = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(client, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(client, endpoint).expect("端点标识");
    let server_endpoint = kernel
        .grant(server, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予服务端句柄");

    let capability =
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE).expect("Capability");
    let frame = kernel
        .create_capability(client, capability, None, Rights::READ | Rights::TRANSFER)
        .expect("创建能力");

    assert!(
        kernel
            .recv(server, server_endpoint)
            .expect("接收")
            .is_none()
    );

    kernel
        .send(
            client,
            endpoint,
            Message::new(4).with_handles([Some(frame), None, None, None]),
        )
        .expect("发送");

    let received = kernel
        .recv(server, server_endpoint)
        .expect("接收")
        .expect("收到消息");
    let transferred = received.message.handles[0].expect("转移的句柄");
    assert!(kernel.handle_id(server, transferred).is_ok());
    assert_eq!(
        kernel.handle_id(client, frame),
        Err(crate::KernelError::Handle(
            hk_handle::HandleError::BadHandle
        ))
    );
    assert_eq!(kernel.handles_live(server), Some(2));
}

#[test]
fn handle_transfer_failure_preserves_sender_and_receiver() {
    let kernel = kernel();
    let client = thread(&kernel);
    let server = kernel
        .spawn_task(HandleTable::new(1), DEFAULT_SPEC)
        .expect("创建服务端任务");

    let endpoint = kernel
        .create_endpoint(client, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(client, endpoint).expect("端点标识");
    let server_endpoint = kernel
        .grant(server, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予服务端句柄");

    let capability =
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE).expect("Capability");
    let frame = kernel
        .create_capability(client, capability, None, Rights::READ | Rights::TRANSFER)
        .expect("创建能力");

    assert!(
        kernel
            .recv(server, server_endpoint)
            .expect("接收")
            .is_none()
    );
    let result = kernel.send(
        client,
        endpoint,
        Message::new(4).with_handles([Some(frame), None, None, None]),
    );
    assert_eq!(
        result,
        Err(crate::KernelError::Handle(
            hk_handle::HandleError::NoHandles
        ))
    );
    assert!(kernel.handle_id(client, frame).is_ok());
    assert_eq!(kernel.handles_live(server), Some(1));

    kernel
        .send(client, endpoint, Message::new(5))
        .expect("重试发送");
    assert_eq!(kernel.state_of(server), Some(ThreadState::Runnable));
    let received = kernel
        .recv(server, server_endpoint)
        .expect("接收")
        .expect("收到消息");
    assert_eq!(received.message.label, 5);
}

#[test]
fn handle_transfer_rejects_duplicate_and_invalid_handles() {
    let kernel = kernel();
    let client = thread(&kernel);
    let server = thread(&kernel);

    let endpoint = kernel
        .create_endpoint(client, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(client, endpoint).expect("端点标识");
    let server_endpoint = kernel
        .grant(server, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予服务端句柄");

    let capability =
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE).expect("Capability");
    let frame = kernel
        .create_capability(client, capability, None, Rights::READ | Rights::TRANSFER)
        .expect("创建能力");

    assert!(
        kernel
            .recv(server, server_endpoint)
            .expect("接收")
            .is_none()
    );
    let before = kernel.handles_live(client);
    let result = kernel.send(
        client,
        endpoint,
        Message::new(4).with_handles([Some(frame), Some(frame), None, None]),
    );
    assert_eq!(
        result,
        Err(crate::KernelError::Handle(
            hk_handle::HandleError::BadHandle
        ))
    );
    assert_eq!(kernel.handles_live(client), before);
    assert!(kernel.handle_id(client, frame).is_ok());

    let bogus = hk_handle::Handle::new(u32::MAX, 0);
    let result = kernel.send(
        client,
        endpoint,
        Message::new(5).with_handles([Some(bogus), None, None, None]),
    );
    assert_eq!(
        result,
        Err(crate::KernelError::Handle(
            hk_handle::HandleError::BadHandle
        ))
    );
    assert_eq!(kernel.handles_live(client), before);
}

#[test]
fn transfers_handles_to_self_without_moving_them() {
    let kernel = kernel();
    let client = thread(&kernel);

    let capability =
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE).expect("Capability");
    let frame = kernel
        .create_capability(client, capability, None, Rights::READ | Rights::TRANSFER)
        .expect("创建能力");

    let mut message = Message::new(1).with_handles([Some(frame), None, None, None]);
    kernel
        .transfer_handles(client, client, &mut message)
        .expect("自转移");
    assert_eq!(message.handles[0], Some(frame));
    assert!(kernel.handle_id(client, frame).is_ok());
}
