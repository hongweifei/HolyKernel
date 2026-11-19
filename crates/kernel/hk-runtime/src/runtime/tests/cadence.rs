//! 策略侧 Cadence/Meter 查询与对象授予测试。

use super::*;

/// 把当前 CPU 的第一个任务设为策略任务，并返回其槽位。
fn policy_caller(runtime: &mut UserRuntime<MockContext, MockPlatform>) -> usize {
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let task = runtime
        .kernel
        .task_of(runtime.tasks[index].tcb)
        .expect("当前 Task");
    runtime
        .kernel
        .set_policy_authority(task, true)
        .expect("策略授权");
    index
}

#[test]
fn cadence_demand_reports_other_realtime_tasks() {
    let mut runtime = runtime();
    let caller = policy_caller(&mut runtime);
    let caller_tcb = runtime.tasks[caller].tcb;
    let caller_task = runtime.kernel.task_of(caller_tcb).expect("调用者 Task");

    let target = runtime
        .tasks
        .iter()
        .enumerate()
        .find(|(slot, task)| *slot != caller && !task.exited)
        .map(|(_, task)| task.tcb)
        .expect("另一个任务");
    let target_task = runtime.kernel.task_of(target).expect("目标 Task");
    let handle = runtime
        .kernel
        .grant_cadence(
            caller_tcb,
            target_task,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::ADMIN,
        )
        .expect("授予 Cadence");
    let mut spec = runtime.kernel.cadence_spec(target_task).expect("目标契约");
    spec.class = hk_cadence::CadenceClass::Realtime;
    spec.budget = hk_cadence::millis(1);
    spec.period = hk_cadence::millis(16);
    spec.deadline = hk_cadence::millis(4);
    runtime
        .kernel
        .update_cadence(caller_tcb, handle, spec)
        .expect("目标改为 Realtime");

    let mut context = MockContext {
        rax: hk_syscall::SYS_CADENCE_DEMAND,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1, "空缓冲查询返回记录数");
    assert!(runtime.platform.written.lock().is_empty(), "查询不写缓冲");

    let mut context = MockContext {
        rax: hk_syscall::SYS_CADENCE_DEMAND,
        rdx: (hk_syscall::CADENCE_DEMAND_RECORD_BYTES * 4) as u64,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1, "只报告一个其它 Realtime 任务");

    let bytes = runtime.platform.written.lock().clone();
    assert_eq!(bytes.len(), hk_syscall::CADENCE_DEMAND_RECORD_BYTES);
    let word = |start: usize| u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap());
    assert_eq!(word(0), hk_cadence::millis(1).ticks());
    assert_eq!(word(8), hk_cadence::millis(16).ticks());
    assert_eq!(word(16), hk_cadence::millis(4).ticks());
    assert_ne!(
        runtime.kernel.task_of(target),
        Some(caller_task),
        "调用者自身不计入需求"
    );
}

#[test]
fn object_grant_named_unlocks_meter_reset() {
    let mut runtime = runtime();
    let caller = policy_caller(&mut runtime);

    let (target, name) = runtime
        .tasks
        .iter()
        .enumerate()
        .find(|(slot, task)| *slot != caller && !task.exited)
        .map(|(_, task)| (task.tcb, task.name.clone()))
        .expect("另一个任务");
    runtime.kernel.charge(target, 123);

    runtime
        .platform
        .input
        .lock()
        .extend_from_slice(name.as_bytes());
    let mut grant = MockContext {
        rax: hk_syscall::SYS_OBJECT_GRANT_NAMED,
        rsi: name.len() as u64,
        rdx: hk_syscall::OBJECT_KIND_METER,
        r10: u64::from(hk_syscall::RIGHT_READ | hk_syscall::RIGHT_ADMIN),
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut grant), Switch::Continue);
    let handle = grant.rax;
    assert_ne!(handle, u64::MAX, "策略任务应按名取得 Meter 句柄");

    let mut reset = MockContext {
        rax: hk_syscall::SYS_METER_RESET,
        rdi: handle,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut reset), Switch::Continue);
    assert_eq!(reset.rax, 0, "Meter ADMIN 句柄使 SYS_METER_RESET 可达");
    assert_eq!(
        runtime
            .kernel
            .task_meter_snapshot(target)
            .expect("计量快照")
            .consumed,
        0
    );
}

#[test]
fn object_grant_named_rejects_non_policy_callers() {
    let mut runtime = runtime();
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let name = runtime
        .tasks
        .iter()
        .enumerate()
        .find(|(slot, task)| *slot != index && !task.exited)
        .map(|(_, task)| task.name.clone())
        .expect("另一个任务");
    runtime
        .platform
        .input
        .lock()
        .extend_from_slice(name.as_bytes());

    let mut grant = MockContext {
        rax: hk_syscall::SYS_OBJECT_GRANT_NAMED,
        rsi: name.len() as u64,
        rdx: hk_syscall::OBJECT_KIND_METER,
        r10: u64::from(hk_syscall::RIGHT_READ),
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut grant), Switch::Continue);
    assert_eq!(grant.rax, u64::MAX, "非策略任务不得按名取句柄");
}
