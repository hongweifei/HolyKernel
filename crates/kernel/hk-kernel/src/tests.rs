//! 内核集成测试。

use hk_device::{DeviceGrant, MmioRange, PciAddress};
use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
use hk_handle::{HandleTable, Rights};
use hk_ipc::Message;
use hk_mem::hmp::{
    Capability, CapabilityKind, Capacity, CapacityId, CapacityKind, FrameBacking, FrameLease,
    Region, RegionFlags,
};

use crate::{Kernel, TcbId, ThreadState, TraceCategory};
use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, DEFAULT_SPEC, millis};

use core::sync::atomic::{AtomicUsize, Ordering};

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
        Capability::new(CapabilityKind::Anonymous, 0x2000, PAGE_SIZE, None).expect("Capability");
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
        .allocate_frame(worker, capacity, PAGE_SIZE)
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
        .split_capacity(worker, capacity, 0x4000, CapacityKind::User)
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
        .allocate_frame(worker, capacity, PAGE_SIZE)
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
        .allocate_frame(worker, capacity, PAGE_SIZE)
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
        .allocate_frame(worker, capacity, PAGE_SIZE)
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
    let capability = Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE, None)
        .expect("能力");
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
fn object_ledger_reads_capability_and_frame() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;

    let capability = Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE, None)
        .expect("能力");
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
        .allocate_frame(worker, capacity_handle, PAGE_SIZE)
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

    let capability = Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE, None)
        .expect("能力");
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

    let capability = Capability::new(
        CapabilityKind::Anonymous,
        (PAGE_SIZE * 2) as u64,
        PAGE_SIZE,
        None,
    )
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
    let capability = Capability::new(
        CapabilityKind::Anonymous,
        (PAGE_SIZE * 2) as u64,
        PAGE_SIZE,
        None,
    )
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
        .allocate_frame(worker, capacity, PAGE_SIZE)
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
        Capability::new(CapabilityKind::Revision, 0x1000, PAGE_SIZE, None).expect("Capability");
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
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE, None).expect("Capability");
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
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE, None).expect("Capability");
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
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE, None).expect("Capability");
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
        Capability::new(CapabilityKind::Anonymous, 0x1000, PAGE_SIZE, None).expect("Capability");
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

#[test]
fn idle_tcb_runs_when_no_work() {
    let kernel = kernel();
    kernel.enable_idle_cpus(1);
    let idle = kernel.schedule(0).expect("idle TCB");
    assert!(kernel.is_idle(idle));
    assert!(!kernel.has_runnable(), "idle 不计入可运行任务");
    assert_eq!(kernel.thread_count(), 1);
    assert_eq!(kernel.idle_thread(0), Some(idle));
}

#[test]
fn real_task_preempts_idle_tcb() {
    let kernel = kernel();
    kernel.enable_idle_cpus(1);
    let idle = kernel.schedule(0).expect("idle TCB");
    let real = thread(&kernel);

    assert_eq!(kernel.schedule(0), Some(real));
    kernel.exit(real).expect("退出真实任务");
    assert_eq!(kernel.schedule(0), Some(idle));
}

#[test]
fn idle_tcb_stays_on_its_cpu() {
    let kernel = kernel();
    kernel.enable_idle_cpus(1);
    assert!(kernel.idle_thread(1).is_none());
    assert_eq!(kernel.schedule(1), None);
}

#[test]
fn distributes_tasks_across_idle_cpus() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let first = thread(&kernel);
    let second = thread(&kernel);

    assert_eq!(kernel.schedule(0), Some(first));
    assert_eq!(kernel.schedule(1), Some(second));
}

#[test]
fn schedules_higher_priority_first() {
    let kernel = kernel();
    let low = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Latency, 1024)
                .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0")),
        )
        .expect("低延迟任务");
    let high = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0")),
        )
        .expect("实时任务");

    assert_eq!(kernel.schedule(0), Some(high));
    assert_eq!(kernel.state_of(high), Some(ThreadState::Running));

    kernel.exit(high).expect("退出实时任务");
    assert_eq!(kernel.schedule(0), Some(low));
}

#[test]
fn rotates_threads_of_equal_priority() {
    let kernel = kernel();
    let pinned = CadenceSpec::new(CadenceClass::Latency, 1024)
        .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0"));
    let first = kernel
        .spawn_task(HandleTable::new(8), pinned)
        .expect("第一个任务");
    let second = kernel
        .spawn_task(HandleTable::new(8), pinned)
        .expect("第二个任务");

    assert_eq!(kernel.schedule(0), Some(first));
    assert_eq!(kernel.schedule(0), Some(second));
    assert_eq!(kernel.schedule(0), Some(first));
}

#[test]
fn child_task_inherits_narrowed_cadence() {
    let kernel = kernel();
    let spec = CadenceSpec::new(CadenceClass::Latency, 2048).with_budget(millis(4), millis(8));
    let parent = kernel
        .spawn_task(HandleTable::new(8), spec)
        .expect("父任务");
    let parent_id = kernel.task_of(parent).expect("父 Task");
    let child = kernel
        .spawn_task_child_parked(HandleTable::new(8), parent_id)
        .expect("子任务");
    let child_id = kernel.task_of(child).expect("子 Task");
    let child_task = kernel.task(child_id).expect("子 Task 存在");
    assert_eq!(child_task.lock().parent(), Some(parent_id));
    let child_spec = kernel.cadence_spec(child_id).expect("子契约");
    assert_eq!(child_spec.weight, 1024);
    assert_eq!(child_spec.budget.ticks(), millis(2).ticks());
    assert_eq!(kernel.cadence_spec(parent_id).expect("父契约").weight, 2048);
}

#[test]
fn parent_exit_promotes_child_cadence() {
    let kernel = kernel();
    let spec = CadenceSpec::new(CadenceClass::Latency, 2048).with_budget(millis(4), millis(8));
    let parent = kernel
        .spawn_task(HandleTable::new(8), spec)
        .expect("父任务");
    let parent_id = kernel.task_of(parent).expect("父 Task");
    let child = kernel
        .spawn_task_child_parked(HandleTable::new(8), parent_id)
        .expect("子任务");
    let child_id = kernel.task_of(child).expect("子 Task");
    let child_cadence = kernel
        .task(child_id)
        .expect("子 Task 存在")
        .lock()
        .cadence();
    assert_eq!(
        child_cadence.lock().parent(),
        Some(hk_cadence::CadenceId::new(parent_id.get()))
    );

    kernel.exit(parent).expect("父任务退出");

    assert!(kernel.task(parent_id).is_none(), "父 Task 应被回收");
    let child_task = kernel.task(child_id).expect("子任务应继续存在");
    assert_eq!(child_task.lock().parent(), None, "子 Task 父链应解除");
    assert_eq!(
        child_cadence.lock().parent(),
        None,
        "子契约应提升为独立契约"
    );
    assert_eq!(child_cadence.lock().weight().get(), 2048 / 2);
    assert_eq!(kernel.state_of(child), Some(ThreadState::Inactive));
}

#[test]
fn realtime_edf_selects_earliest_absolute_deadline() {
    let kernel = kernel();
    let late = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(16))
                .with_deadline(millis(8)),
        )
        .expect("晚截止期任务");
    let early = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(16))
                .with_deadline(millis(4)),
        )
        .expect("早截止期任务");

    assert_eq!(
        kernel.queue_entry(late).map(|entry| entry.1),
        Some(8_000),
        "绝对截止期 = period_start + deadline"
    );
    assert_eq!(kernel.queue_entry(early).map(|entry| entry.1), Some(4_000));
    assert_eq!(kernel.schedule(0), Some(early), "EDF 应先选最早截止期");
}

#[test]
fn affinity_without_online_cpu_stays_unrunnable() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let spec = CadenceSpec::new(CadenceClass::Latency, 1024)
        .with_affinity(hk_cadence::CpuMask::single(3).expect("CPU 3"));
    let thread = kernel.spawn_task(HandleTable::new(8), spec).expect("任务");
    assert_eq!(kernel.state_of(thread), Some(ThreadState::Runnable));
    assert!(!kernel.has_runnable(), "无可用 CPU 时不应进入队列");
    assert_eq!(kernel.schedule(0), None);
    assert_eq!(kernel.schedule(1), None);
}

#[test]
fn affinity_wait_is_counted_after_cpu_becomes_allowed() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let thread = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Latency, 1024)
                .with_affinity(hk_cadence::CpuMask::single(3).expect("CPU 3")),
        )
        .expect("任务");
    let task = kernel.task_of(thread).expect("Task");
    kernel.set_policy_authority(task, true).expect("策略权限");
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;
    let handle = kernel
        .grant_cadence(thread, task, rights)
        .expect("契约句柄");

    kernel.set_clock(1_000);
    let mut spec = kernel.cadence_spec(task).expect("契约");
    spec.affinity = hk_cadence::CpuMask::single(0).expect("CPU 0");
    kernel
        .update_cadence(thread, handle, spec)
        .expect("亲和变更");

    assert!(kernel.schedule(0).is_some(), "亲和变更后应可运行");
    let meter = kernel.task_meter_snapshot(thread).expect("计量");
    assert!(meter.waited >= 1_000, "无在线亲和 CPU 的等待应计入 Meter");
}

#[test]
fn next_deadline_includes_queued_realtime_deadline() {
    let kernel = kernel();
    kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1)),
        )
        .expect("实时任务");
    assert_eq!(
        kernel.next_deadline(0),
        Some(millis(1).ticks()),
        "一次性定时器应包含最早 Realtime 截止期"
    );
}

#[test]
fn affinity_change_rehomes_running_task() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let thread = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Latency, 1024),
        )
        .expect("任务");
    let task = kernel.task_of(thread).expect("Task");
    kernel.set_policy_authority(task, true).expect("策略权限");
    let cadence = kernel
        .grant_cadence(thread, task, Rights::READ | Rights::WRITE | Rights::ADMIN)
        .expect("授予 Cadence");

    assert_eq!(kernel.schedule(0), Some(thread), "初始在 CPU0 运行");
    let mut spec = kernel.cadence_spec(task).expect("契约");
    spec.affinity = hk_cadence::CpuMask::single(1).expect("CPU 1");
    kernel
        .update_cadence(thread, cadence, spec)
        .expect("收紧亲和性");

    // Grant 结束后按新亲和性重新入队；CPU0 不再选中它，CPU1 可以。
    assert_eq!(kernel.schedule(0), None);
    assert_eq!(kernel.schedule(1), Some(thread));
}

#[test]
fn requests_resched_for_higher_priority_wake() {
    RESCHED_CALLS.store(0, Ordering::SeqCst);
    let kernel = kernel();
    let pinned = CadenceSpec::new(CadenceClass::Throughput, 1024)
        .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0"));
    let low = kernel
        .spawn_task(HandleTable::new(8), pinned)
        .expect("低优先级任务");
    assert_eq!(kernel.schedule(0), Some(low));

    kernel.set_resched_hook(count_resched);
    let _high = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1))
                .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0")),
        )
        .expect("高优先级任务");
    assert!(
        RESCHED_CALLS.load(Ordering::SeqCst) > 0,
        "更弱的当前任务应触发 resched"
    );
}

#[test]
fn task_serializes_threads_and_rotates_them() {
    let kernel = kernel();
    let spec = CadenceSpec::new(CadenceClass::Latency, 1024)
        .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0"));
    let first = kernel
        .spawn_task(HandleTable::new(8), spec)
        .expect("主线程");
    let task = kernel.task_of(first).expect("任务");
    let second = kernel.spawn_thread(task).expect("第二线程");

    let a = kernel.schedule(0).expect("首个 Grant");
    assert!(a == first || a == second);
    assert_eq!(
        kernel.schedule(1),
        None,
        "同一 Task 不能同时获得第二个 Grant"
    );
    let b = kernel.schedule(0).expect("轮转");
    assert_ne!(a, b);
    assert_eq!(kernel.schedule(0).expect("轮转回"), a);
}

#[test]
fn parked_task_with_parent_promotes_when_parent_exits() {
    let kernel = kernel();
    let parent_thread = thread(&kernel);
    let parent = kernel.task_of(parent_thread).expect("父 Task");
    let child = kernel
        .spawn_task_parked_with_parent(HandleTable::new(8), DEFAULT_SPEC, Some(parent))
        .expect("子 Task");
    let child_task = kernel.task_of(child).expect("子 Task 存在");
    assert_eq!(
        kernel.task(child_task).expect("Task").lock().parent(),
        Some(parent)
    );
    kernel.activate(child).expect("激活子 Task");
    kernel.exit(parent_thread).expect("父 Task 退出");
    assert_eq!(
        kernel.task(child_task).expect("Task").lock().parent(),
        None,
        "父退出后子任务提升为独立契约"
    );
}

#[test]
fn threads_share_task_handle_table() {
    let kernel = kernel();
    let first = kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("主线程");
    let task = kernel.task_of(first).expect("任务");
    let second = kernel.spawn_thread(task).expect("第二线程");

    let endpoint = kernel
        .create_endpoint(first, Rights::READ | Rights::WRITE)
        .expect("端点");
    let endpoint_id = kernel.handle_id(first, endpoint).expect("对象标识");
    assert_eq!(
        kernel.handle_id(second, endpoint),
        Ok(endpoint_id),
        "线程共享主线程的句柄编号"
    );

    let notification = kernel
        .create_notification(second, Rights::READ)
        .expect("通知");
    assert!(
        kernel.handle_id(first, notification).is_ok(),
        "线程创建的句柄对主线程可见"
    );
    assert_eq!(
        kernel.handles_live(first),
        kernel.handles_live(second),
        "共享表的在用句柄数一致"
    );
}

#[test]
fn task_budget_is_shared_by_threads() {
    let kernel = kernel();
    let spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(1), millis(4))
        .with_deadline(millis(2))
        .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0"));
    let first = kernel
        .spawn_task(HandleTable::new(8), spec)
        .expect("主线程");
    let task = kernel.task_of(first).expect("任务");
    let _second = kernel.spawn_thread(task).expect("第二线程");

    let running = kernel.schedule(0).expect("Grant");
    kernel.charge(running, millis(1).ticks() + 1);
    assert_eq!(kernel.schedule(0), None, "预算耗尽后整个 Task 节流");
    kernel.set_clock(millis(4).ticks());
    assert!(kernel.schedule(0).is_some(), "周期补充后恢复一个 Grant");
}

#[test]
fn records_waited_switches_and_overruns() {
    let kernel = kernel();
    let spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(1), millis(4))
        .with_deadline(millis(1))
        .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0"));
    let thread = kernel
        .spawn_task(HandleTable::new(8), spec)
        .expect("实时任务");
    kernel.set_clock(millis(2).ticks());
    assert!(kernel.schedule(0).is_some());

    let meter = kernel.task_meter_snapshot(thread).expect("计量");
    assert_eq!(meter.waited, millis(2).ticks());
    assert_eq!(meter.switches, 1);
    assert_eq!(meter.overruns, 1);
}

#[test]
fn throttles_budgeted_task_and_refills_next_period() {
    let kernel = kernel();
    let realtime = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(2)),
        )
        .expect("实时任务");

    assert_eq!(kernel.schedule(0), Some(realtime));
    kernel.charge(realtime, millis(1).ticks() + 1);
    assert_eq!(kernel.schedule(0), None);

    kernel.set_clock(millis(4).ticks());
    assert_eq!(kernel.schedule(0), Some(realtime));
}

#[test]
fn shares_cpu_in_weight_proportion() {
    let kernel = kernel();
    let heavy = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Throughput, 2048)
                .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0")),
        )
        .expect("高权重任务");
    let light = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Throughput, 1024)
                .with_affinity(hk_cadence::CpuMask::single(0).expect("CPU 0")),
        )
        .expect("低权重任务");

    let mut heavy_runs = 0_u32;
    let mut light_runs = 0_u32;
    for _ in 0..30 {
        let next = kernel.schedule(0).expect("可调度任务");
        kernel.charge(next, 10);
        if next == heavy {
            heavy_runs += 1;
        } else {
            assert_eq!(next, light);
            light_runs += 1;
        }
    }
    assert!(
        heavy_runs.abs_diff(light_runs * 2) <= 2,
        "权重比 2:1 的运行次数异常：{heavy_runs}:{light_runs}"
    );
}

#[test]
fn grants_cadence_and_meter_handles() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;
    let cadence = kernel
        .grant_cadence(thread, task, rights)
        .expect("授予 Cadence");
    let meter = kernel
        .grant_meter(thread, task, rights)
        .expect("授予 Meter");

    assert_eq!(
        kernel.handle_kind(thread, cadence),
        Some(hk_handle::ObjectKind::Cadence)
    );
    assert_eq!(
        kernel.handle_kind(thread, meter),
        Some(hk_handle::ObjectKind::Meter)
    );
    let spec = kernel.cadence_spec(task).expect("契约快照");
    assert_eq!(spec.class, CadenceClass::Latency);
}

#[test]
fn update_cadence_rejects_escalation_without_policy_authority() {
    let kernel = kernel();
    let parent = thread(&kernel);
    let parent_task = kernel.task_of(parent).expect("父任务");
    let child = kernel
        .spawn_task_child_parked(HandleTable::with_default_quota(), parent_task)
        .expect("子任务");
    let child_task = kernel.task_of(child).expect("子任务编号");
    let cadence = kernel
        .grant_cadence(
            child,
            child_task,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )
        .expect("授予 Cadence");

    let mut escalate_class = kernel.cadence_spec(child_task).expect("契约");
    escalate_class.class = CadenceClass::Realtime;
    assert_eq!(
        kernel.update_cadence(child, cadence, escalate_class),
        Err(crate::KernelError::CadenceDenied),
        "非策略任务不能修改类别"
    );

    let mut escalate_weight = kernel.cadence_spec(child_task).expect("契约");
    escalate_weight.weight = 100_000;
    assert_eq!(
        kernel.update_cadence(child, cadence, escalate_weight),
        Err(crate::KernelError::CadenceDenied),
        "非策略任务不能超过父契约权重"
    );

    let mut narrowed = kernel.cadence_spec(child_task).expect("契约");
    narrowed.weight = 64;
    kernel
        .update_cadence(child, cadence, narrowed)
        .expect("父范围内的权重调整应被允许");
    assert_eq!(kernel.cadence_spec(child_task).expect("契约").weight, 64);
}

#[test]
fn cadence_switch_restarts_realtime_deadline_at_now() {
    let kernel = kernel();
    kernel.set_clock(50_000);
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    kernel.set_policy_authority(task, true).expect("策略权限");
    let cadence = kernel
        .grant_cadence(thread, task, Rights::READ | Rights::WRITE | Rights::ADMIN)
        .expect("授予 Cadence");

    let mut spec = kernel.cadence_spec(task).expect("契约");
    spec.class = CadenceClass::Realtime;
    spec.budget = millis(1);
    spec.period = millis(4);
    spec.deadline = millis(1);
    kernel
        .update_cadence(thread, cadence, spec)
        .expect("切换为 Realtime");

    assert_eq!(
        kernel.queue_entry(thread),
        Some((CadenceClass::Realtime, 51_000)),
        "新契约的绝对截止期应从当前时刻起算"
    );
}

#[test]
fn attaches_and_regrants_named_cadence() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;
    let cadence = kernel
        .grant_cadence(thread, task, rights)
        .expect("授予 Cadence");

    kernel
        .attach_cadence_to_task(thread, task, cadence)
        .expect("按 Task 附加契约");
    let before = kernel.cadence_snapshot(thread, cadence).expect("契约快照");
    let epoch = kernel
        .regrant_cadence_to_task(thread, task, cadence)
        .expect("撤销并重新授予");
    assert_eq!(epoch, before.epoch + 1);
    assert!(
        kernel.cadence_snapshot(thread, cadence).is_err(),
        "重授予后旧句柄必须失效"
    );
    let renewed = kernel
        .grant_cadence(thread, task, rights)
        .expect("重新授予 Cadence");
    let after = kernel.cadence_snapshot(thread, renewed).expect("契约快照");
    assert_eq!(after.epoch, epoch);
    kernel
        .set_policy_authority(task, true)
        .expect("授予策略权限");
    assert!(kernel.is_policy_authority(thread));
}

#[test]
fn spawned_task_with_parent_promotes_when_parent_exits() {
    let kernel = kernel();
    let parent_thread = thread(&kernel);
    let parent = kernel.task_of(parent_thread).expect("父 Task");
    let child = kernel
        .spawn_task_with_cadence_and_parent(
            HandleTable::new(8),
            Cadence::from_spec(CadenceId::next(), DEFAULT_SPEC).expect("契约"),
            Some(parent),
        )
        .expect("子 Task");
    let child_task = kernel.task_of(child).expect("子 Task 存在");
    assert_eq!(
        kernel.task(child_task).expect("Task").lock().parent(),
        Some(parent)
    );
    kernel.exit(parent_thread).expect("父 Task 退出");
    assert_eq!(
        kernel.task(child_task).expect("Task").lock().parent(),
        None,
        "父退出后应提升为独立契约"
    );
}

#[test]
fn updates_splits_revokes_and_reattaches_cadence() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;
    let cadence = kernel
        .grant_cadence(thread, task, rights)
        .expect("授予 Cadence");
    let task_handle = kernel.grant_task(thread, task, rights).expect("授予 Task");

    let mut spec = kernel.cadence_spec(task).expect("契约");
    spec.weight = 2048;
    kernel
        .update_cadence(thread, cadence, spec)
        .expect("更新契约");
    assert_eq!(kernel.cadence_spec(task).expect("契约").weight, 2048);

    let child = kernel
        .split_cadence(thread, cadence, 512, 0)
        .expect("派生子契约");
    let snapshot = kernel.cadence_snapshot(thread, child).expect("子契约快照");
    assert_eq!(snapshot.weight, 512);
    assert_eq!(snapshot.parent, task.get());

    kernel
        .attach_cadence(thread, task_handle, child)
        .expect("重新附加");
    assert_eq!(kernel.cadence_spec(task).expect("契约").weight, 512);

    let epoch = kernel.revoke_cadence(thread, child).expect("撤销契约");
    assert_eq!(epoch, 1);
    assert_eq!(kernel.schedule(0), None);
    // 撤销使既有句柄在解析时失效；需要由 Root/Tempo 重新授予。
    assert!(kernel.cadence_snapshot(thread, child).is_err());
}

#[test]
fn suspended_task_is_not_revived_by_wake() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    let rights = Rights::READ | Rights::WRITE | Rights::ADMIN;
    let cadence = kernel
        .grant_cadence(thread, task, rights)
        .expect("授予 Cadence");
    kernel.park(thread).expect("阻塞");
    kernel.revoke_cadence(thread, cadence).expect("撤销契约");
    kernel.wake_thread(thread);
    assert_eq!(kernel.schedule(0), None, "Suspended 不能被普通唤醒复活");
}

#[test]
fn meter_reset_requires_admin_and_clears_counters() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    let read_only = kernel
        .grant_meter(thread, task, Rights::READ)
        .expect("只读 Meter");
    assert!(kernel.reset_meter(thread, read_only).is_err());

    let admin = kernel
        .grant_meter(thread, task, Rights::READ | Rights::ADMIN)
        .expect("管理 Meter");
    kernel.charge(thread, 10);
    assert!(kernel.meter_snapshot(thread, admin).expect("快照").consumed > 0);
    kernel.reset_meter(thread, admin).expect("重置");
    assert_eq!(
        kernel.meter_snapshot(thread, admin).expect("快照").consumed,
        0
    );
}

#[test]
fn policy_task_can_split_and_attach_own_cadence() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    kernel
        .set_policy_authority(task, true)
        .expect("授予策略授权");

    let pool = kernel.self_cadence_handle(thread).expect("策略句柄");
    let child = kernel
        .split_cadence(thread, pool, 512, 0)
        .expect("在池内派生");
    kernel
        .attach_own_cadence(thread, child)
        .expect("附加到自身");
    assert_eq!(kernel.cadence_spec(task).expect("契约").weight, 512);
}

#[test]
fn lends_caller_contract_to_server_and_restores() {
    let kernel = kernel();
    let caller_spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(2), millis(16))
        .with_deadline(millis(8));
    let caller = kernel
        .spawn_task(HandleTable::new(8), caller_spec)
        .expect("调用方");
    let server = kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("服务端");
    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(caller, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予端点");

    kernel
        .call(caller, client_endpoint, Message::new(1))
        .expect("调用");
    assert_eq!(kernel.lending_depth(server), 0);
    let received = kernel
        .recv(server, endpoint)
        .expect("接收")
        .expect("收到调用");
    let token = received.reply.expect("回复令牌");
    assert_eq!(kernel.lending_depth(server), 1);
    assert_eq!(
        kernel.queue_entry(server).map(|entry| entry.0),
        Some(CadenceClass::Realtime),
        "服务端借用调用方的实时类别"
    );

    kernel.charge(server, 1_000);
    assert_eq!(
        kernel
            .task_meter_snapshot(caller)
            .expect("调用方计量")
            .consumed,
        1_000
    );
    assert_eq!(
        kernel.task_metrics(server).expect("服务端计量").consumed,
        1_000
    );

    kernel.charge(server, 1_500);
    kernel.reply(server, token, Message::new(2)).expect("回复");
    assert_eq!(kernel.lending_depth(server), 0);
    kernel.charge(server, 500);
    assert_eq!(
        kernel
            .task_meter_snapshot(caller)
            .expect("调用方计量")
            .consumed,
        2_500
    );
    assert_eq!(
        kernel.task_metrics(server).expect("服务端计量").consumed,
        3_000
    );
}

fn lending_pair(
    kernel: &Kernel,
    caller_spec: CadenceSpec,
    server_spec: CadenceSpec,
) -> (TcbId, TcbId) {
    let caller = kernel
        .spawn_task(HandleTable::new(8), caller_spec)
        .expect("调用方");
    let server = kernel
        .spawn_task(HandleTable::new(8), server_spec)
        .expect("服务端");
    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(caller, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予端点");
    kernel
        .call(caller, client_endpoint, Message::new(1))
        .expect("调用");
    let received = kernel
        .recv(server, endpoint)
        .expect("接收")
        .expect("收到调用");
    assert!(received.reply.is_some(), "调用应携带回复令牌");
    (caller, server)
}

fn lending_server(kernel: &Kernel, caller_spec: CadenceSpec, server_spec: CadenceSpec) -> TcbId {
    lending_pair(kernel, caller_spec, server_spec).1
}

#[test]
fn lending_throttles_caller_while_server_continues() {
    let kernel = kernel();
    let caller_spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(1), millis(16))
        .with_deadline(millis(8));
    let (caller, server_tcb) = lending_pair(&kernel, caller_spec, DEFAULT_SPEC);

    // 调用方预算 1 ms：服务端执行 1 ms 后调用方 Throttled，服务端仍继续记账。
    kernel.charge(server_tcb, 1_000);
    assert_eq!(
        kernel
            .task_meter_snapshot(caller)
            .expect("调用方计量")
            .throttled,
        1,
        "调用方预算耗尽应进入 Throttled"
    );
    assert_eq!(
        kernel
            .task_metrics(server_tcb)
            .expect("服务端计量")
            .consumed,
        1_000,
        "服务端自身计量继续累计"
    );
}

#[test]
fn lending_uses_caller_contract_when_class_is_stronger() {
    let kernel = kernel();
    let caller = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(2), millis(16))
        .with_deadline(millis(8));
    let server = CadenceSpec::new(CadenceClass::Latency, 128);

    let server_tcb = lending_server(&kernel, caller, server);

    assert_eq!(
        kernel.queue_entry(server_tcb).map(|entry| entry.0),
        Some(CadenceClass::Realtime),
        "实时调用方的类别更强"
    );
    assert_eq!(
        kernel.slice_for_thread(server_tcb),
        Some(2_000),
        "应使用调用方更强契约的时间片"
    );
}

#[test]
fn lending_uses_stronger_key_within_same_class() {
    let kernel = kernel();
    let caller = CadenceSpec::new(CadenceClass::Latency, 2048);
    let server = CadenceSpec::new(CadenceClass::Latency, 128).with_latency(millis(4));

    let server_tcb = lending_server(&kernel, caller, server);

    assert_eq!(
        kernel.queue_entry(server_tcb),
        Some((CadenceClass::Latency, 0)),
        "同类别下调用方虚拟截止期更小"
    );
    assert_eq!(
        kernel.slice_for_thread(server_tcb),
        Some(1_000),
        "应使用调用方更强契约的时间片"
    );
}

#[test]
fn cancel_call_releases_lending_and_discards_late_reply() {
    let kernel = kernel();
    let caller_spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(2), millis(16))
        .with_deadline(millis(8));
    let caller = kernel
        .spawn_task(HandleTable::new(8), caller_spec)
        .expect("调用方");
    let server = kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("服务端");
    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(caller, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予端点");
    kernel
        .call(caller, client_endpoint, Message::new(1))
        .expect("调用");
    let received = kernel
        .recv(server, endpoint)
        .expect("接收")
        .expect("收到调用");
    let token = received.reply.expect("回复令牌");
    assert_eq!(kernel.lending_depth(server), 1);

    assert!(kernel.cancel_call(caller));
    assert_eq!(kernel.lending_depth(server), 0);
    kernel
        .reply(server, token, Message::new(2))
        .expect("迟到回复应被丢弃");
    assert_eq!(
        kernel.queue_entry(server).map(|entry| entry.0),
        Some(CadenceClass::Latency)
    );
}

#[test]
fn disabled_lending_does_not_charge_caller() {
    let kernel = kernel();
    let caller_spec =
        CadenceSpec::new(CadenceClass::Latency, 1024).with_lending(hk_cadence::Lending::disabled());
    let caller = kernel
        .spawn_task(HandleTable::new(8), caller_spec)
        .expect("调用方");
    let server = kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("服务端");
    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(caller, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予端点");

    kernel
        .call(caller, client_endpoint, Message::new(1))
        .expect("调用");
    let _ = kernel
        .recv(server, endpoint)
        .expect("接收")
        .expect("收到调用");
    assert_eq!(kernel.lending_depth(server), 0);
    kernel.charge(server, 1_000);
    assert_eq!(
        kernel
            .task_meter_snapshot(caller)
            .expect("调用方计量")
            .consumed,
        0
    );
}

#[test]
fn respects_cpu_affinity() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let pinned = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Latency, 1024)
                .with_affinity(hk_cadence::CpuMask::single(1).expect("CPU 1")),
        )
        .expect("任务");
    assert_eq!(kernel.schedule(0), None);
    assert_eq!(kernel.schedule(1), Some(pinned));
}

#[test]
fn pinned_task_stays_on_first_cpu() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let fixed = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Latency, 1024).with_migratable(false),
        )
        .expect("任务");
    assert_eq!(kernel.schedule(0), Some(fixed));
    kernel.block(fixed, crate::BlockReason::Park).expect("停泊");
    kernel.wake_thread(fixed);
    assert_eq!(kernel.schedule(1), None);
    assert_eq!(kernel.schedule(0), Some(fixed));
}

#[test]
fn records_migration_when_picked_by_another_cpu() {
    let kernel = kernel();
    kernel.set_online_cpus(2);
    let task = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Latency, 1024),
        )
        .expect("任务");
    assert_eq!(kernel.schedule(0), Some(task));
    kernel.block(task, crate::BlockReason::Park).expect("停泊");
    kernel.wake_thread(task);
    assert_eq!(kernel.schedule(1), Some(task));
    let meter = kernel.task_meter_snapshot(task).expect("计量");
    assert_eq!(meter.migrations, 1);
    assert!(meter.migration_cost > 0);
}

#[test]
fn restores_meter_watermark() {
    let kernel = kernel();
    let thread = thread(&kernel);
    let task = kernel.task_of(thread).expect("任务存在");
    let before = kernel.task_meter_snapshot(thread).expect("计量");
    kernel.charge(thread, 1234);
    assert!(kernel.task_meter_snapshot(thread).expect("计量").consumed >= 1234);
    kernel.restore_meter(task, before).expect("恢复计量");
    assert_eq!(
        kernel.task_meter_snapshot(thread).expect("计量").consumed,
        before.consumed
    );
}

#[test]
fn exit_releases_handles_and_objects() {
    let kernel = kernel();
    let worker = thread(&kernel);
    kernel
        .create_endpoint(worker, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    assert_eq!(kernel.live_objects(), 1);

    kernel.exit(worker).expect("退出线程");

    assert_eq!(kernel.thread_count(), 0);
    assert_eq!(kernel.live_objects(), 0);
    assert_eq!(kernel.collect_garbage(), 1);
    assert_eq!(
        kernel.handle_id(worker, hk_handle::Handle::new(0, 0)),
        Err(crate::KernelError::UnknownThread(worker))
    );
}

#[test]
fn closes_handles_from_thread_table() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let endpoint = kernel
        .create_endpoint(worker, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    assert_eq!(kernel.handles_live(worker), Some(1));

    kernel.close_handle(worker, endpoint).expect("关闭句柄");
    assert_eq!(kernel.handles_live(worker), Some(0));
    assert!(kernel.handle_id(worker, endpoint).is_err());
}

#[test]
fn dump_reports_threads_and_trace() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let notification = kernel
        .create_notification(worker, Rights::READ | Rights::WRITE)
        .expect("创建通知");
    assert_eq!(kernel.wait(worker, notification, 0b1).expect("等待"), None);

    let report = kernel.dump();
    assert!(report.contains("kernel: threads=1"), "{report}");
    assert!(report.contains("tcb=1"), "{report}");
    assert!(report.contains("state=Blocked"), "{report}");
    assert!(report.contains("trace: events="), "{report}");
}

#[test]
fn creates_device_grant_object() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let grant = DeviceGrant {
        address: PciAddress::new(0, 1, 0, 0),
        vendor: 0x1AF4,
        device: 0x1001,
        mmio: [
            Some(MmioRange {
                base: 0xFE00_0000,
                len: 0x1000,
            }),
            None,
            None,
            None,
            None,
            None,
        ],
        io: [None; 6],
        dma: None,
        irq: Some(11),
        capabilities: [0x09; 8],
        capability_count: 1,
    };
    kernel
        .create_device_grant(worker, grant, Rights::READ | Rights::WRITE)
        .expect("创建设备授权");
    assert_eq!(kernel.live_objects(), 1);
    assert_eq!(kernel.handles_live(worker), Some(1));
}

#[test]
fn signals_notification_by_object_id() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let notification = kernel
        .create_notification(worker, Rights::READ | Rights::WRITE)
        .expect("创建通知");
    let id = kernel.handle_id(worker, notification).expect("通知标识");

    kernel.signal_id(id, 0b100).expect("发送通知");
    assert_eq!(
        kernel.wait(worker, notification, 0b100).expect("等待通知"),
        Some(0b100)
    );
}
