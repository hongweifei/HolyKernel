//! 句柄转移与接收方配额测试。

use super::*;

#[test]
fn transfers_handle_over_ipc() {
    let mut runtime = runtime();

    let Step::Resume { index: sender, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let mut context = MockContext {
        rax: SYS_YIELD,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume {
        index: sender_again,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(sender_again, sender);
    context.rax = SYS_SEND_HANDLE;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    context.r10 = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(runtime.tasks()[receiver].state, "runnable");

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, receiver);
    assert_eq!(context.rax, 9);
    assert_eq!(context.rdx, 5);
    assert_eq!(context.rcx, 1);
    assert_eq!(context.r8, (EXTRA_HANDLE_BASE + 1) as u64);
    assert_eq!(runtime.tasks()[receiver].handles, 6 + ARENA_HANDLES);
}

#[test]
fn ipc_send_rejected_when_receiver_quota_exhausted() {
    let mut runtime = runtime();

    let Step::Resume { index: sender, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let mut context = MockContext {
        rax: SYS_YIELD,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, sender);
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(104),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x88_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(capacity_handle));
    let frame = runtime
        .kernel
        .allocate_frame(tcb, capacity_handle, hk_hal::PAGE_SIZE, None)
        .expect("Frame");
    let number = runtime
        .install_extra_capability(index, frame)
        .expect("安装 Frame");

    // 把接收方配额填到只剩 2 KiB，无法容纳一页 Frame。
    let limit = runtime.process(receiver).ledger.limit();
    runtime
        .process_mut(receiver)
        .ledger
        .charge(limit - 2048)
        .expect("填满配额");
    let handles = runtime.tasks()[receiver].handles;

    context.rax = SYS_SEND_HANDLE;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    context.r10 = number;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(
        context.rax,
        u64::MAX,
        "接收方配额不足时发送必须失败，消息不能入队"
    );
    assert_eq!(
        runtime.tasks()[receiver].handles,
        handles,
        "失败的转移不得留下句柄槽位"
    );
    assert_eq!(
        runtime.process(receiver).ledger.charged(),
        limit - 2048,
        "失败的转移不得改变接收方账本"
    );
    assert!(
        runtime.handle_at(index, number as usize).is_some(),
        "发送失败时句柄仍归发送方"
    );
    let _ = runtime.finish(0, sender);
}

#[test]
fn ipc_queued_handle_waits_for_receiver_quota() {
    let mut runtime = runtime();

    // 发送方先把消息连句柄投递进端点队列（接收方未阻塞）。
    let Step::Resume {
        index: sender,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[sender].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(107),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x8e_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[sender].capacities.push(Some(capacity_handle));
    let frame = runtime
        .kernel
        .allocate_frame(tcb, capacity_handle, hk_hal::PAGE_SIZE, None)
        .expect("Frame");
    let number = runtime
        .install_extra_capability(sender, frame)
        .expect("安装 Frame");

    context.rax = SYS_SEND_HANDLE;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    context.r10 = number;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "无接收者时消息应入队");

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(receiver, 1);

    // 接收方配额不足：接收失败，但消息必须留在队列里。
    let limit = runtime.process(receiver).ledger.limit();
    let filler = runtime
        .process_mut(receiver)
        .ledger
        .charge(limit - 2048)
        .expect("填满配额");
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "配额不足时接收必须失败");

    // 释放配额后重试：同一条消息与句柄仍可收到。
    runtime
        .process_mut(receiver)
        .ledger
        .release(filler)
        .expect("释放配额");
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 9, "配额恢复后应取到原消息");
    assert_ne!(context.r8, 0, "句柄必须随消息一并交付");
    let _ = runtime.finish(0, receiver);
}
