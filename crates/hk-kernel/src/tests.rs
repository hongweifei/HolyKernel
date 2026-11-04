//! 内核集成测试。

use hk_hal::{DeviceGrant, MmioRange, PAGE_SIZE, PciAddress, PhysAddr, PhysRange};
use hk_handle::{HandleTable, Rights};
use hk_ipc::Message;

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

    let untyped = kernel
        .create_untyped(
            client,
            PhysRange::new(PhysAddr::new(0x10_0000), 0x10_000),
            Rights::READ | Rights::WRITE,
        )
        .expect("创建 Untyped");
    let frame = kernel
        .allocate_frame(client, untyped, PAGE_SIZE, Rights::READ | Rights::TRANSFER)
        .expect("切分页帧");

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
