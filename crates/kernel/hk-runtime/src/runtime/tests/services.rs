//! 服务启动、系统调用分派与调度测试。

use super::*;

#[test]
fn memory_service_receives_user_capacity_subset() {
    let runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    assert_eq!(
        runtime.tasks[1].context.rsi, 0x5001,
        "Root Server 应把初始 Capacity 句柄显式传给服务"
    );
    let granted = runtime.tasks[1]
        .capacities
        .iter()
        .flatten()
        .next()
        .and_then(|handle| {
            runtime
                .kernel
                .capacity_ref(runtime.tasks[1].tcb, *handle)
                .ok()
        })
        .expect("Memory Server 持有容量");
    let granted = granted.lock();
    assert_eq!(granted.kind(), hk_mem::hmp::CapacityKind::User);
    assert_eq!(granted.limit(), 0x20_0000);
}

#[test]
fn memory_service_capability_allocates_from_user_capacity() {
    let mut runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    // 让第一个任务阻塞，调度到 Memory Server。
    context.rax = SYS_RECV;
    context.rdi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    let Step::Resume {
        index: memory,
        mut context,
    } = runtime.step(0)
    else {
        panic!("Memory Server 应可运行");
    };
    assert_eq!(memory, 1);

    let user_handle = runtime.tasks[memory]
        .capacities
        .iter()
        .flatten()
        .next()
        .copied()
        .expect("User Capacity 句柄");
    let user = runtime
        .kernel
        .capacity_ref(runtime.tasks[memory].tcb, user_handle)
        .expect("User Capacity");
    let user_range = user.lock().range();

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX, "能力创建应成功");
    let (_, _, backing) = runtime
        .capability_at(memory, capability as usize)
        .expect("能力句柄");
    let backing = backing.expect("能力有常驻后备");
    assert!(
        backing.start() >= user_range.start() && backing.end() <= user_range.end(),
        "能力页必须来自 Memory Server 的 User Capacity"
    );
    let _ = runtime.finish(0, memory);
}

#[test]
fn dispatches_alloc_and_exit() {
    let mut runtime = runtime();
    assert_eq!(runtime.task_count(), 2);

    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_ALLOC;
    context.rdi = 64;
    context.rsi = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0x10_0000);

    context.rax = SYS_EXIT;
    context.rdi = 3;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Exit);
    assert_eq!(runtime.finish(0, index), Some((String::from("a"), 3)));
}

#[test]
fn framebuffer_access_is_display_only() {
    let mut runtime = crate::boot::user_runtime(
        &[
            crate::ServiceSpec::new("hk-server-display"),
            crate::ServiceSpec::new("other"),
        ],
        MockPlatform::new(),
        |_index, endpoint| MockContext {
            rdi: endpoint,
            ..MockContext::default()
        },
    )
    .expect("装配运行时");
    for index in 0..runtime.task_count() {
        runtime.attach(
            index,
            MockContext {
                rdi: index as u64 + 1,
                ..MockContext::default()
            },
        );
    }

    let Step::Resume { index: display, .. } = runtime.step(0) else {
        panic!("显示服务应可运行");
    };
    let mut context = MockContext {
        rax: SYS_FRAMEBUFFER_INFO,
        rdi: 0x1000,
        rsi: 64,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_syscall::FRAMEBUFFER_DESCRIPTOR_BYTES as u64);

    context.rax = SYS_FRAMEBUFFER_MAP;
    context.rdi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 16);

    // 非显示服务不得访问帧缓冲。
    context.rax = SYS_YIELD;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    runtime.finish(0, display);
    let Step::Resume { index: other, .. } = runtime.step(0) else {
        panic!("普通服务应可运行");
    };
    assert_ne!(other, display);
    let mut context = MockContext {
        rax: SYS_FRAMEBUFFER_INFO,
        rdi: 0x1000,
        rsi: 64,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    context.rax = SYS_FRAMEBUFFER_MAP;
    context.rdi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
}

#[test]
fn yielded_task_is_rescheduled_after_finish() {
    let mut runtime = runtime();
    let Step::Resume {
        index: first,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    context.rax = SYS_YIELD;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.finish(0, first), None);

    // yield 不是永久阻塞：后续调度不能返回空闲，且原任务会再次被选中。
    let mut resumed = false;
    for _ in 0..4 {
        let Step::Resume { index, mut context } = runtime.step(0) else {
            panic!("yield 后不应判定空闲");
        };
        if index == first {
            resumed = true;
        }
        context.rax = SYS_YIELD;
        assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
        runtime.finish(0, index);
    }
    assert!(resumed, "yield 的任务应被重新调度");
}
