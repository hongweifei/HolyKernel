//! 内核 IPC：send/recv、call/reply、通知与句柄转移测试。

#![allow(clippy::wildcard_imports)]

use super::*;

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
