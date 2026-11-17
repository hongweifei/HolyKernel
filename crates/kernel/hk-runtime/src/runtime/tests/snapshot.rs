//! Cadence/Meter 视图、检查点与唤醒测试。

use super::*;

#[test]
fn renders_cadence_meter_and_tempo_views() {
    let runtime = runtime();
    let cadence = runtime.cadence_text("a");
    assert!(cadence.contains("cadence a"), "{cadence}");
    assert!(cadence.contains("weight="), "{cadence}");
    let meter = runtime.meter_text("a");
    assert!(meter.contains("meter a"), "{meter}");
    assert!(meter.contains("consumed="), "{meter}");
    assert!(runtime.cadence_text("missing").contains("未知任务"));
    assert!(runtime.tempo_text().contains("Tempo 未运行"));
}

#[test]
fn checkpoint_restore_preserves_cadence_and_meter() {
    let mut runtime = runtime();
    let info = runtime
        .tasks()
        .into_iter()
        .find(|task| task.name == "a")
        .expect("任务 a");
    let tcb = hk_kernel::TcbId::new(info.tcb);
    let task = runtime.kernel.task_of(tcb).expect("Task");
    runtime
        .kernel
        .set_policy_authority(task, true)
        .expect("策略权限");
    let rights = hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::ADMIN;
    let cadence = runtime
        .kernel
        .grant_cadence(tcb, task, rights)
        .expect("授予 Cadence");
    let mut spec = runtime.kernel.cadence_spec(task).expect("契约");
    spec.weight = 2048;
    spec.latency = hk_cadence::millis(2);
    runtime
        .kernel
        .update_cadence(tcb, cadence, spec)
        .expect("更新契约");
    runtime.kernel.charge(tcb, 1_234);

    let before = runtime.kernel.task_cadence_snapshot(tcb).expect("契约快照");
    let before_meter = runtime.kernel.task_meter_snapshot(tcb).expect("计量快照");

    let slot = runtime
        .tasks()
        .iter()
        .position(|task| task.name == "a")
        .expect("任务槽");
    runtime.checkpoint("a").expect("保存检查点");
    let restored = runtime.restore_checkpoint("a").expect("恢复检查点");
    assert_eq!(restored, slot, "应恢复到同一任务槽");

    let info = runtime
        .tasks()
        .into_iter()
        .find(|task| task.name == "a" && task.state != "exited")
        .expect("恢复后的任务");
    let tcb = hk_kernel::TcbId::new(info.tcb);
    let after = runtime.kernel.task_cadence_snapshot(tcb).expect("契约快照");
    let after_meter = runtime.kernel.task_meter_snapshot(tcb).expect("计量快照");

    assert_eq!(after.class, before.class);
    assert_eq!(after.weight, before.weight);
    assert_eq!(after.budget, before.budget);
    assert_eq!(after.period, before.period);
    assert_eq!(after.deadline, before.deadline);
    assert_eq!(after.latency, before.latency);
    assert_eq!(after_meter.consumed, before_meter.consumed);
    assert_eq!(after_meter.waited, before_meter.waited);
    assert_eq!(after_meter.throttled, before_meter.throttled);
    assert_eq!(after_meter.switches, before_meter.switches);
    assert_eq!(after_meter.overruns, before_meter.overruns);
    assert_eq!(after_meter.migrations, before_meter.migrations);
    assert_eq!(after_meter.migration_cost, before_meter.migration_cost);
}

#[test]
fn checkpoint_preserves_region_offset() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0x1000;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    let offset = runtime
        .process(index)
        .regions
        .iter()
        .flatten()
        .next()
        .map(|region| region.offset);
    assert_eq!(offset, Some(0x1000));

    // 检查点要求任务不在任何 CPU 上执行。
    runtime.current[0] = None;
    runtime.checkpoint("a").expect("保存检查点");
    runtime.restore_checkpoint("a").expect("恢复检查点");

    let restored = runtime
        .process(index)
        .regions
        .iter()
        .flatten()
        .next()
        .map(|region| region.offset);
    assert_eq!(restored, Some(0x1000), "恢复必须保留 Region 的能力内偏移");
    let _ = runtime.finish(0, index);
}

#[test]
fn restart_grants_device_service_its_dma_capacity() {
    let mut runtime = runtime();
    let name = "hk-device-restart";
    let slot = runtime.reuse_slot(name);
    runtime
        .start_at_with_regions(
            slot,
            name,
            true,
            hk_cadence::DEFAULT_SPEC,
            MockContext::default(),
            None,
            &[],
        )
        .expect("启动设备服务");
    let process = runtime.process(slot);
    let tcb = runtime.tasks[slot].tcb;
    let (_, kind, _, limit, _) = runtime
        .kernel
        .capacity_info(tcb, process.allocation_capacity)
        .expect("容量信息");
    assert_eq!(kind, hk_mem::hmp::CapacityKind::Dma);
    assert_eq!(limit, crate::DEVICE_CAPACITY_BYTES as u64);

    runtime.terminate(name, 0);
    let slot = runtime.reuse_slot(name);
    runtime
        .start_at_with_regions(
            slot,
            name,
            true,
            hk_cadence::DEFAULT_SPEC,
            MockContext::default(),
            None,
            &[],
        )
        .expect("重启设备服务");
    let process = runtime.process(slot);
    let tcb = runtime.tasks[slot].tcb;
    let (_, kind, _, limit, _) = runtime
        .kernel
        .capacity_info(tcb, process.allocation_capacity)
        .expect("重启后的容量信息");
    assert_eq!(kind, hk_mem::hmp::CapacityKind::Dma);
    assert_eq!(limit, crate::DEVICE_CAPACITY_BYTES as u64);
}

#[test]
fn checkpoint_restores_threads_and_regions() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_THREAD_CREATE;
    context.rdi = 0x4000;
    context.rsi = 0x8000;
    context.rdx = 0;
    context.r10 = 0x7000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let thread = context.rax;
    assert_ne!(thread, u64::MAX);

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region_handle = context.rax;
    assert_ne!(region_handle, u64::MAX);
    let before = runtime
        .process(index)
        .regions
        .iter()
        .flatten()
        .find(|region| region.len == 0x2000)
        .expect("已登记 Region");
    let (virt, len, rights) = (before.virt, before.len, before.rights);
    let _ = runtime.finish(0, index);

    runtime.checkpoint("a").expect("保存检查点");
    let slot = runtime.restore_checkpoint("a").expect("恢复检查点");
    assert_eq!(slot, index);

    let restored: Vec<_> = runtime
        .tasks()
        .into_iter()
        .filter(|task| task.state != "exited" && task.name.starts_with('a'))
        .collect();
    assert_eq!(restored.len(), 2, "主线程与附加线程都应恢复");

    let region = runtime
        .process(slot)
        .regions
        .iter()
        .flatten()
        .find(|region| region.virt == virt)
        .expect("恢复 Region");
    assert_eq!(region.len, len);
    assert_eq!(region.rights, rights);
}

#[test]
fn enumerates_owned_objects() {
    let platform = MockPlatform::new();
    let written = Arc::clone(&platform.written);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    context.rax = hk_syscall::SYS_OBJECT_ENUM;
    context.rdi = 0;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let count = context.rax;
    assert!(count >= 2, "至少包含 Capacity 与能力记录");

    context.rax = hk_syscall::SYS_OBJECT_ENUM;
    context.rdi = 0;
    context.rsi = 0x1000;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 64);
    let record = written.lock();
    assert_eq!(record.len(), 64);
    assert_eq!(record[0], 0, "第一条记录是 Capacity");
    let _ = runtime.finish(0, index);
}

#[test]
fn checkpoint_records_object_graph() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let _ = runtime.finish(0, index);

    let size = runtime.checkpoint("a").expect("保存检查点");
    let image = runtime.checkpoint_chunk("a", 0, size).expect("读取快照");
    let snapshot = hk_checkpoint::Snapshot::decode(&image).expect("解码快照");
    assert!(!snapshot.objects.is_empty(), "快照应记录对象图");
    assert!(snapshot.objects.iter().any(|object| object.kind == 2));
    runtime.restore_checkpoint("a").expect("恢复检查点");
}

#[test]
fn reports_self_cadence_and_meter() {
    let mut runtime = runtime();
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CADENCE_INFO;
    context.rdi = 0;
    context.rsi = 0x1000;
    context.rdx = hk_cadence::CadenceSnapshot::ENCODED_LEN as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_cadence::CadenceSnapshot::ENCODED_LEN as u64);

    context.rax = hk_syscall::SYS_METER_INFO;
    context.rdx = hk_cadence::MeterSnapshot::ENCODED_LEN as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_cadence::MeterSnapshot::ENCODED_LEN as u64);
}

#[test]
fn arms_wakeup_at_sleep_deadline() {
    let mut runtime = runtime();
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = hk_syscall::SYS_SLEEP;
    context.rdi = 3;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    // 契约时钟按 timer_ticks=7 折算为 70 000 µs，再休眠 3 个 10 ms tick。
    assert_eq!(runtime.next_wakeup(0), Some(100_000));
}
