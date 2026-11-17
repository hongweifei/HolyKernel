//! 调度、idle TCB、亲和性与预算轮转测试。

use super::*;

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
