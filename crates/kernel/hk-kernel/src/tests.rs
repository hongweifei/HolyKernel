//! 内核集成测试。

use hk_device::{DeviceGrant, MmioRange, PciAddress};
use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
use hk_handle::{HandleTable, Rights};
use hk_ipc::Message;
use hk_mem::hmp::{
    Arena, ArenaId, Capability, CapabilityKind, Capacity, CapacityId, CapacityKind, Ledger,
    LedgerId, Region, RegionFlags,
};

use crate::{Kernel, Priority, TcbId, ThreadState, TraceCategory};

fn kernel() -> Kernel {
    Kernel::new()
}

fn thread(kernel: &Kernel) -> TcbId {
    kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL)
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
    let ledger = Ledger::new(LedgerId::new(1), 0x10_000);
    let arena = Arena::new(ArenaId::new(1), VirtAddr::new(0x40_0000), 0x10_000).expect("Arena");

    kernel
        .create_capacity(worker, capacity, rights)
        .expect("创建 Capacity");
    kernel
        .create_arena(worker, arena, rights)
        .expect("创建 Arena");
    kernel
        .create_frame(worker, frame, rights)
        .expect("创建 Frame");
    kernel
        .create_capability(worker, capability, None, rights)
        .expect("创建 Capability");
    kernel
        .create_region(worker, region, rights)
        .expect("创建 Region");
    kernel
        .create_ledger(worker, ledger, rights)
        .expect("创建 Ledger");

    assert_eq!(kernel.live_objects(), 6);
    assert_eq!(kernel.handles_live(worker), Some(6));
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
    assert_eq!(kernel.share_frame(worker, frame), Ok(2));
    assert_eq!(kernel.pin_frame(worker, frame, true), Ok(()));
    assert_eq!(
        kernel.free_frame(worker, capacity, frame).err(),
        Some(crate::KernelError::Hmp(
            hk_mem::hmp::HmpError::PermissionDenied
        ))
    );
    assert_eq!(kernel.pin_frame(worker, frame, false), Ok(()));
    assert_eq!(kernel.release_frame(worker, frame), Ok(1));
    assert_eq!(kernel.free_frame(worker, capacity, frame), Ok(()));
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
fn releasing_last_reference_returns_frame_to_capacity() {
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

    assert_eq!(kernel.share_frame(worker, frame), Ok(2));
    assert_eq!(kernel.release_frame(worker, frame), Ok(1));
    assert_eq!(
        kernel
            .capacity_info(worker, capacity)
            .expect("Capacity 信息")
            .4,
        PAGE_SIZE as u64,
        "仍有引用时不归还"
    );

    assert_eq!(kernel.release_frame(worker, frame), Ok(0));
    assert_eq!(
        kernel
            .capacity_info(worker, capacity)
            .expect("Capacity 信息")
            .4,
        0,
        "最后一个引用归还 Capacity"
    );
    assert!(matches!(
        kernel.frame_info(worker, frame),
        Err(crate::KernelError::Hmp(hk_mem::hmp::HmpError::NotMapped))
    ));

    // 句柄保留但负载已空；关闭它不会再次归还。
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
fn tracks_arena_regions_and_frame_refs() {
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
    let capability = Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE, None)
        .expect("Capability");
    let mut region = Region::new(
        &capability,
        0,
        PAGE_SIZE,
        Rights::READ | Rights::WRITE,
        RegionFlags::empty(),
    )
    .expect("Region");
    region.map(VirtAddr::new(0x40_0000)).expect("映射");

    let arena = kernel
        .create_arena(
            worker,
            Arena::new(ArenaId::new(2), VirtAddr::new(0x40_0000), 0x10_000).expect("Arena"),
            rights,
        )
        .expect("创建 Arena");
    let frame = kernel
        .create_frame(worker, frame, rights)
        .expect("创建 Frame");

    let region_id = kernel
        .arena_insert_region(worker, arena, &region)
        .expect("登记 Region");
    assert_eq!(kernel.arena_region_count(worker, arena), Ok(1));
    assert_eq!(kernel.frame_refs(worker, frame), Ok(1));
    assert_eq!(kernel.share_frame(worker, frame), Ok(2));
    assert_eq!(kernel.pin_frame(worker, frame, true), Ok(()));
    assert!(kernel.frame_range(worker, frame).is_ok());

    kernel
        .arena_remove_region(worker, arena, region_id)
        .expect("移除 Region");
    assert_eq!(kernel.arena_region_count(worker, arena), Ok(0));
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
    let server = kernel.spawn_thread(HandleTable::new(1), Priority::NORMAL);

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
fn schedules_higher_priority_first() {
    let kernel = kernel();
    let low = kernel.spawn_thread(HandleTable::new(8), Priority::LOWEST);
    let high = kernel.spawn_thread(HandleTable::new(8), Priority::HIGHEST);

    assert_eq!(kernel.schedule(0), Some(high));
    assert_eq!(kernel.state_of(high), Some(ThreadState::Running));

    kernel.exit(high).expect("退出高优先级线程");
    assert_eq!(kernel.schedule(0), Some(low));
}

#[test]
fn rotates_threads_of_equal_priority() {
    let kernel = kernel();
    let first = thread(&kernel);
    let second = thread(&kernel);

    assert_eq!(kernel.schedule(0), Some(first));
    assert_eq!(kernel.schedule(0), Some(second));
    assert_eq!(kernel.schedule(0), Some(first));
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
