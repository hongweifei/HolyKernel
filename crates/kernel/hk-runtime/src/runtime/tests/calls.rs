//! 通知等待、call/reply 与超时测试。

use super::*;
use crate::runtime::oom::MEMORY_EVENT_OOM;
use hk_syscall::SYS_MEMORY_EVENT_SUBSCRIBE;

#[test]
fn waits_for_notification() {
    let mut runtime = runtime();

    let Step::Resume {
        index: waiter,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(waiter, 0);
    context.rax = SYS_WAIT_NOTIFY;
    context.rdi = 1;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[waiter].pending, "notify");

    let Step::Resume {
        index: signaler,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(signaler, 1);
    context.rax = SYS_NOTIFY;
    context.rdi = 1;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, waiter);
    assert_eq!(context.rax, 1);
    assert_eq!(runtime.tasks()[waiter].pending, "-");
}

#[test]
fn calls_and_replies() {
    let mut runtime = runtime();

    let Step::Resume {
        index: caller,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(caller, 0);
    context.rax = SYS_CALL;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[caller].pending, "call");

    let Step::Resume {
        index: server,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(server, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 9);
    assert_eq!(context.rdx, 5);
    assert_eq!(context.rcx, 1);

    context.rax = SYS_REPLY;
    context.rdi = 10;
    context.rsi = 6;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, caller);
    assert_eq!(context.rax, 10);
    assert_eq!(context.rdx, 6);
    assert_eq!(context.rcx, 2);
    assert_eq!(runtime.tasks()[caller].pending, "-");
}

#[test]
fn call_reply_transfers_handle_to_caller() {
    let mut runtime = runtime();

    let Step::Resume {
        index: caller,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CALL;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[caller].pending, "call");

    let Step::Resume {
        index: server,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(server, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rcx, 1, "调用方编号");

    // 服务端把自己的通知句柄随回复转移给调用方。
    context.rax = SYS_SELF_NOTIFICATION;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let transferred = context.rax;
    assert_ne!(transferred, u64::MAX);
    context.rax = SYS_REPLY_HANDLE;
    context.rdi = 10;
    context.rsi = 6;
    context.rdx = transferred;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, caller);
    assert_eq!(context.rax, 10);
    assert_eq!(context.rdx, 6);
    assert_ne!(context.r8, 0, "调用方应收到可用的句柄编号");

    let mut close = context;
    close.rax = SYS_HANDLE_CLOSE;
    close.rdi = close.r8;
    assert_eq!(runtime.dispatch(0, &mut close), Switch::Continue);
    assert_eq!(close.rax, 0);
    let _ = runtime.finish(0, caller);
}

#[test]
fn times_out_unanswered_call() {
    let mut runtime = runtime();

    let Step::Resume {
        index: caller,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(caller, 0);
    context.rax = SYS_CALL_TIMEOUT;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    context.r10 = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[caller].pending, "call");

    // deadline 立即到期：wake_timeouts 取消挂起调用并唤醒调用方。
    let mut resumed = None;
    for _ in 0..6 {
        match runtime.step(0) {
            Step::Resume { index, context } if index == caller => {
                resumed = Some(context);
                break;
            }
            Step::Resume { mut context, .. } => {
                // 让服务端睡下，确保调用方获得调度。
                context.rax = SYS_SLEEP;
                context.rdi = 1;
                let _ = runtime.dispatch(0, &mut context);
            }
            _ => break,
        }
    }

    let context = resumed.expect("调用方应被超时唤醒");
    assert_eq!(context.rax, u64::MAX);
    assert_eq!(runtime.tasks()[caller].pending, "-");
}

#[test]
fn memory_events_notify_subscriber() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let (slot, notification) = runtime.tasks[index]
        .notifications
        .iter()
        .enumerate()
        .find_map(|(slot, handle)| handle.map(|handle| (slot, handle)))
        .expect("自己的通知");

    context.rax = SYS_MEMORY_EVENT_SUBSCRIBE;
    context.rdi = slot as u64 + 1;
    assert_eq!(runtime.dispatch(index, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    runtime.notify_memory_event(MEMORY_EVENT_OOM);
    assert_eq!(
        runtime
            .kernel
            .wait(tcb, notification, 1 << MEMORY_EVENT_OOM)
            .expect("等待通知"),
        Some(1 << MEMORY_EVENT_OOM)
    );

    context.rax = SYS_MEMORY_EVENT_SUBSCRIBE;
    context.rdi = 0;
    assert_eq!(runtime.dispatch(index, &mut context), Switch::Continue);
    assert_eq!(runtime.oom.event_notification, None);
}
