//! Cadence/Meter 授予、借用、亲和与对象生命周期测试。

use super::*;

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

#[test]
fn provider_grants_capability_by_id() {
    let kernel = kernel();
    let owner = thread(&kernel);
    let other = thread(&kernel);
    let endpoint = kernel
        .create_endpoint(owner, Rights::READ | Rights::WRITE)
        .expect("端点");
    let capability = kernel
        .create_capability(
            owner,
            Capability::new(CapabilityKind::Anonymous, PAGE_SIZE as u64, PAGE_SIZE).expect("能力"),
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )
        .expect("能力句柄");
    kernel
        .bind_capability_provider(owner, capability, endpoint, ProviderKind::Anonymous)
        .expect("绑定 Provider");
    let id = kernel.handle_id(owner, capability).expect("对象标识");
    let task = kernel.task_of(owner).expect("Provider Task");
    let sibling = kernel.spawn_thread(task).expect("同 Task 线程");
    assert!(
        kernel
            .grant_capability(sibling, id, Rights::READ | Rights::WRITE)
            .is_ok(),
        "同一 Task 的回收线程应能代 Provider 取句柄"
    );

    assert!(
        kernel.grant_capability(other, id, Rights::READ).is_err(),
        "非 Provider 任务不能按标识取句柄"
    );
    let granted = kernel
        .grant_capability(
            owner,
            id,
            Rights::READ | Rights::TRANSFER | Rights::ADMIN | Rights::WRITE,
        )
        .expect("Provider 取句柄");
    let rights = kernel
        .handle_rights(owner, granted, ObjectKind::Capability)
        .expect("句柄权利");
    assert!(rights.contains(Rights::READ | Rights::TRANSFER));
    assert!(rights.contains(Rights::WRITE));
    assert!(!rights.contains(Rights::ADMIN), "授权永不含 ADMIN");
}

#[test]
fn capacity_reports_bound_node() {
    let kernel = kernel();
    let worker = thread(&kernel);
    let capacity = Capacity::new(
        CapacityId::next(),
        PhysRange::new(PhysAddr::new(0x50_0000), 0x10_000),
        PAGE_SIZE,
        CapacityKind::Numa(3),
        0,
    )
    .expect("NUMA Capacity");
    let handle = kernel
        .create_capacity(worker, capacity, Rights::READ | Rights::ADMIN)
        .expect("Capacity 句柄");
    assert_eq!(kernel.capacity_node(worker, handle).expect("节点"), Some(3));
}
