//! 仿真器测试。

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_cadence::{CadenceClass, CadenceSpec, DEFAULT_SPEC, millis};
use hk_handle::{HandleTable, Rights};
use hk_ipc::Message;
use hk_kernel::Kernel;

use crate::{RunReport, SimError, Simulator, ThreadStep};

#[test]
fn runs_send_and_recv_to_completion() {
    let kernel = Kernel::new();
    let server = kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务");
    let client = kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务");

    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(client, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予客户端句柄");

    let mut simulator = Simulator::new(kernel, 7);
    simulator.register(
        server,
        Box::new(
            move |kernel, tid| match kernel.recv(tid, endpoint).expect("接收") {
                Some(received) => {
                    assert_eq!(received.message.label, 1);
                    ThreadStep::Exit
                }
                None => ThreadStep::Continue,
            },
        ),
    );
    simulator.register(
        client,
        Box::new(move |kernel, tid| {
            kernel
                .send(tid, client_endpoint, Message::new(1))
                .expect("发送");
            ThreadStep::Exit
        }),
    );

    let report = simulator.run().expect("运行完成");
    assert_eq!(report.exits, 2);
    assert_eq!(report.steps, 3);
    assert_eq!(report.seed, 7);
    assert!(report.trace_events >= 3);
}

fn ping_pong(seed: u64) -> RunReport {
    let kernel = Kernel::new();
    let server = kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务");
    let client = kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务");

    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(client, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予客户端句柄");

    let mut simulator = Simulator::new(kernel, seed);
    simulator.register(
        server,
        Box::new(
            move |kernel, tid| match kernel.recv(tid, endpoint).expect("接收") {
                Some(received) => {
                    if let Some(token) = received.reply {
                        kernel.reply(tid, token, Message::new(99)).expect("回复");
                    }
                    ThreadStep::Exit
                }
                None => ThreadStep::Continue,
            },
        ),
    );
    let mut requested = false;
    simulator.register(
        client,
        Box::new(move |kernel, tid| {
            if requested {
                assert_eq!(kernel.take_reply(tid), Some(Message::new(99)));
                ThreadStep::Exit
            } else {
                kernel
                    .call(tid, client_endpoint, Message::new(7))
                    .expect("调用");
                requested = true;
                ThreadStep::Continue
            }
        }),
    );

    simulator.run().expect("运行完成")
}

#[test]
fn replays_identically_with_same_seed() {
    let first = ping_pong(42);
    let second = ping_pong(42);

    assert_eq!(first, second);
    assert_eq!(first.exits, 2);
}

#[test]
fn detects_deadlock() {
    let kernel = Kernel::new();
    let worker = kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("创建任务");
    let endpoint = kernel
        .create_endpoint(worker, Rights::READ | Rights::WRITE)
        .expect("创建端点");

    let mut simulator = Simulator::new(kernel, 1);
    simulator.register(
        worker,
        Box::new(move |kernel, tid| {
            kernel.recv(tid, endpoint).expect("接收");
            ThreadStep::Continue
        }),
    );

    assert_eq!(simulator.run(), Err(SimError::Deadlock { blocked: 1 }));
}

#[test]
fn enforces_step_limit() {
    let kernel = Kernel::new();
    let worker = kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("创建任务");

    let mut simulator = Simulator::new(kernel, 1).with_step_limit(5);
    simulator.register(worker, Box::new(|_, _| ThreadStep::Continue));

    assert_eq!(simulator.run(), Err(SimError::StepLimit { steps: 6 }));
}

#[test]
#[should_panic(expected = "线程缺少执行体")]
fn panics_on_thread_without_body() {
    let kernel = Kernel::new();
    kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("创建任务");

    let mut simulator = Simulator::new(kernel, 1);
    let _ = simulator.run();
}

#[test]
fn replays_heavy_task_twice_as_often() {
    let kernel = Kernel::new();
    let heavy = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Throughput, 2048),
        )
        .expect("高权重任务");
    let light = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("低权重任务");

    let mut simulator = Simulator::new(kernel, 5).with_quantum(10);
    let heavy_steps = Arc::new(AtomicU64::new(0));
    let light_steps = Arc::new(AtomicU64::new(0));
    let (heavy_counter, light_counter) = (heavy_steps.clone(), light_steps.clone());
    let calls = Arc::new(AtomicU64::new(0));
    let stop = calls.clone();
    simulator.register(
        heavy,
        Box::new(move |_, _| {
            heavy_counter.fetch_add(1, Ordering::Relaxed);
            if stop.fetch_add(1, Ordering::Relaxed) >= 29 {
                ThreadStep::Exit
            } else {
                ThreadStep::Continue
            }
        }),
    );
    let stop = calls.clone();
    simulator.register(
        light,
        Box::new(move |_, _| {
            light_counter.fetch_add(1, Ordering::Relaxed);
            if stop.load(Ordering::Relaxed) >= 30 {
                ThreadStep::Exit
            } else {
                ThreadStep::Continue
            }
        }),
    );
    let report = simulator.run().expect("运行完成");
    let heavy_runs = heavy_steps.load(Ordering::Relaxed);
    let light_runs = light_steps.load(Ordering::Relaxed);
    let heavy_in_report = report.order.iter().filter(|id| **id == heavy).count() as u64;
    assert!(
        heavy_in_report.abs_diff(light_runs * 2) <= 2,
        "权重比 2:1 的回放顺序异常：{heavy_in_report}:{light_runs}（heavy 累计 {heavy_runs}）"
    );
}

#[test]
fn replays_realtime_before_throughput() {
    let kernel = Kernel::new();
    let background = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("后台任务");
    let realtime = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1)),
        )
        .expect("实时任务");

    let realtime_runs = Arc::new(AtomicU64::new(0));
    let counter = realtime_runs.clone();
    let mut simulator = Simulator::new(kernel, 9).with_quantum(100);
    simulator.register(
        realtime,
        Box::new(move |_, _| {
            counter.fetch_add(1, Ordering::Relaxed);
            ThreadStep::Exit
        }),
    );
    let counter = realtime_runs.clone();
    simulator.register(
        background,
        Box::new(move |_, _| {
            if counter.load(Ordering::Relaxed) > 0 {
                ThreadStep::Exit
            } else {
                ThreadStep::Continue
            }
        }),
    );

    let report = simulator.run().expect("运行完成");
    // 实时类先于吞吐类被选择；随后后台任务才运行。
    assert_eq!(report.order, alloc::vec![realtime, background]);
}

#[test]
fn replays_budget_throttling() {
    let kernel = Kernel::new();
    let realtime = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1)),
        )
        .expect("实时任务");

    let mut simulator = Simulator::new(kernel, 11)
        .with_quantum(300)
        .with_step_limit(40);
    simulator.register(realtime, Box::new(|_, _| ThreadStep::Continue));
    let report = simulator.run();
    assert_eq!(report, Err(SimError::StepLimit { steps: 41 }));

    // 1 ms 预算在每步 300 µs 下第 4 步耗尽；随后时钟跳到 4 ms 补充点再次入队。
    let meter = simulator
        .kernel()
        .task_meter_snapshot(realtime)
        .expect("计量快照");
    assert!(meter.throttled > 0, "预算耗尽应进入 Throttled");
    assert!(
        meter.consumed >= 1_200,
        "恢复后应继续累计执行时间，实际 {}",
        meter.consumed
    );
}

#[test]
fn replays_periodic_refill_after_throttle() {
    let kernel = Kernel::new();
    let realtime = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1)),
        )
        .expect("实时任务");
    let background = kernel
        .spawn_task(
            HandleTable::new(8),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("后台任务");

    let mut simulator = Simulator::new(kernel, 13)
        .with_quantum(300)
        .with_step_limit(30);
    simulator.register(realtime, Box::new(|_, _| ThreadStep::Continue));
    simulator.register(background, Box::new(|_, _| ThreadStep::Continue));
    // 后台任务在实时任务节流期间获得 CPU；到补充点后实时任务重新获得 Grant。
    assert_eq!(simulator.run(), Err(SimError::StepLimit { steps: 31 }));
    let meter = simulator
        .kernel()
        .task_meter_snapshot(realtime)
        .expect("计量快照");
    assert!(meter.throttled > 0);
    assert!(meter.consumed > 1_000, "补充后应继续运行");
}

fn lending_replay(seed: u64) -> RunReport {
    let kernel = Kernel::new();
    let caller_spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
        .with_budget(millis(1), millis(4))
        .with_deadline(millis(1));
    let caller = kernel
        .spawn_task(HandleTable::with_default_quota(), caller_spec)
        .expect("调用方");
    let server = kernel
        .spawn_task(HandleTable::with_default_quota(), DEFAULT_SPEC)
        .expect("服务端");

    let endpoint = kernel
        .create_endpoint(server, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(server, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(caller, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予客户端句柄");

    let mut simulator = Simulator::new(kernel, seed).with_quantum(50);
    simulator.register(
        server,
        Box::new(
            move |kernel, tid| match kernel.recv(tid, endpoint).expect("接收") {
                Some(received) => {
                    assert!(
                        kernel.lending_depth(tid) > 0,
                        "服务端收到调用后必须持有 lending frame"
                    );
                    if let Some(token) = received.reply {
                        kernel.reply(tid, token, Message::new(2)).expect("回复");
                        assert_eq!(kernel.lending_depth(tid), 0, "回复后必须弹出 lending frame");
                    }
                    ThreadStep::Exit
                }
                None => ThreadStep::Continue,
            },
        ),
    );
    let mut requested = false;
    simulator.register(
        caller,
        Box::new(move |kernel, tid| {
            if requested {
                assert_eq!(kernel.take_reply(tid), Some(Message::new(2)));
                ThreadStep::Exit
            } else {
                kernel
                    .call(tid, client_endpoint, Message::new(1))
                    .expect("调用");
                requested = true;
                ThreadStep::Continue
            }
        }),
    );

    simulator.run().expect("lending 回放完成")
}

#[test]
fn replays_lending_deterministically() {
    let first = lending_replay(21);
    let second = lending_replay(21);
    assert_eq!(first, second, "相同种子应产生相同 lending 回放");
    assert_eq!(first.exits, 2);
}

fn migration_replay(seed: u64) -> (RunReport, u64) {
    let kernel = Kernel::new();
    let spec = CadenceSpec::new(CadenceClass::Throughput, 1024);
    let first = kernel
        .spawn_task(HandleTable::with_default_quota(), spec)
        .expect("任务 1");
    let second = kernel
        .spawn_task(HandleTable::with_default_quota(), spec)
        .expect("任务 2");
    let third = kernel
        .spawn_task(HandleTable::with_default_quota(), spec)
        .expect("任务 3");

    let mut simulator = Simulator::new(kernel, seed)
        .with_cpus(2)
        .with_quantum(10)
        .with_step_limit(200);
    let migrations = Arc::new(AtomicU64::new(0));
    for thread in [first, second, third] {
        let runs = Arc::new(AtomicU64::new(0));
        let observed = migrations.clone();
        simulator.register(
            thread,
            Box::new(move |kernel, tid| {
                if let Some(meter) = kernel.task_meter_snapshot(tid) {
                    observed.fetch_max(meter.migrations, Ordering::Relaxed);
                }
                if runs.fetch_add(1, Ordering::Relaxed) >= 2 {
                    ThreadStep::Exit
                } else {
                    ThreadStep::Continue
                }
            }),
        );
    }

    let report = simulator.run().expect("迁移回放完成");
    (report, migrations.load(Ordering::Relaxed))
}

#[test]
fn replays_migration_deterministically() {
    let (first, first_migrations) = migration_replay(33);
    let (second, second_migrations) = migration_replay(33);
    assert_eq!(first, second, "相同种子应产生相同迁移回放");
    assert_eq!(first_migrations, second_migrations);
    assert!(first_migrations > 0, "多核回放应产生跨核迁移");
}

#[test]
fn realtime_task_meets_edf_deadlines() {
    let kernel = Kernel::new();
    let realtime = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(8))
                .with_deadline(millis(4)),
        )
        .expect("实时任务");
    let mut simulator = Simulator::new(kernel, 7)
        .with_quantum(100)
        .with_step_limit(5_000);
    let runs = Arc::new(AtomicU64::new(0));
    let overruns = Arc::new(AtomicU64::new(u64::MAX));
    let throttled = Arc::new(AtomicU64::new(0));
    let overruns_body = Arc::clone(&overruns);
    let throttled_body = Arc::clone(&throttled);
    simulator.register(
        realtime,
        Box::new(move |kernel, tid| {
            if runs.fetch_add(1, Ordering::Relaxed) >= 100 {
                if let Some(meter) = kernel.task_meter_snapshot(tid) {
                    overruns_body.store(meter.overruns, Ordering::Relaxed);
                    throttled_body.store(meter.throttled, Ordering::Relaxed);
                }
                ThreadStep::Exit
            } else {
                ThreadStep::Continue
            }
        }),
    );

    let report = simulator.run().expect("实时回放完成");
    assert_eq!(report.exits, 1);
    assert_eq!(
        overruns.load(Ordering::Relaxed),
        0,
        "EDF 调度不应错过绝对截止期"
    );
    assert!(
        throttled.load(Ordering::Relaxed) > 0,
        "1 ms 预算应在 8 ms 周期内节流"
    );
}

#[test]
fn replays_wakeup_preemption() {
    let kernel = Kernel::new();
    let realtime = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1)),
        )
        .expect("实时任务");
    let background = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("后台任务");
    let client = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("客户端");

    let endpoint = kernel
        .create_endpoint(realtime, Rights::READ | Rights::WRITE)
        .expect("创建端点");
    let endpoint_id = kernel.handle_id(realtime, endpoint).expect("端点标识");
    let client_endpoint = kernel
        .grant(client, endpoint_id, Rights::READ | Rights::WRITE)
        .expect("授予客户端句柄");

    let mut simulator = Simulator::new(kernel, 17)
        .with_quantum(10)
        .with_step_limit(400);
    simulator.register(
        realtime,
        Box::new(
            move |kernel, tid| match kernel.recv(tid, endpoint).expect("接收") {
                Some(_) => ThreadStep::Exit,
                None => ThreadStep::Continue,
            },
        ),
    );
    let background_runs = Arc::new(AtomicU64::new(0));
    simulator.register(
        background,
        Box::new(move |_, _| {
            if background_runs.fetch_add(1, Ordering::Relaxed) >= 20 {
                ThreadStep::Exit
            } else {
                ThreadStep::Continue
            }
        }),
    );
    simulator.register(
        client,
        Box::new(move |kernel, tid| {
            kernel
                .send(tid, client_endpoint, Message::new(1))
                .expect("发送");
            ThreadStep::Exit
        }),
    );

    let report = simulator.run().expect("唤醒抢占回放完成");
    let client_position = report
        .order
        .iter()
        .position(|thread| *thread == client)
        .expect("客户端应被调度");
    assert_eq!(
        report.order.get(client_position + 1),
        Some(&realtime),
        "发送唤醒后实时任务应立即抢占后台任务"
    );
    assert!(report.resched_requests > 0, "唤醒抢占应请求 reschedule IPI");
}

#[test]
fn replays_bounded_wakeup_response() {
    let kernel = Kernel::new();
    let realtime = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(1)),
        )
        .expect("实时任务");
    let hog = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("后台任务");

    let mut simulator = Simulator::new(kernel, 5)
        .with_quantum(10)
        .with_step_limit(200);
    let parked = Arc::new(AtomicU64::new(0));
    let first = parked.clone();
    simulator.register(
        realtime,
        Box::new(move |kernel, tid| {
            if first.fetch_add(1, Ordering::Relaxed) == 0 {
                kernel.park(tid).expect("挂起");
                ThreadStep::Continue
            } else {
                ThreadStep::Exit
            }
        }),
    );
    let runs = Arc::new(AtomicU64::new(0));
    simulator.register(
        hog,
        Box::new(move |kernel, _| {
            let count = runs.fetch_add(1, Ordering::Relaxed);
            if count == 2 {
                kernel.wake_thread(realtime);
            }
            if count >= 6 {
                ThreadStep::Exit
            } else {
                ThreadStep::Continue
            }
        }),
    );

    let report = simulator.run().expect("唤醒回放完成");
    assert_eq!(report.exits, 2);
    assert!(
        report.max_wait <= 10,
        "唤醒到运行应在一个量子内有界，实际 {}",
        report.max_wait
    );
}

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn stress_replay(seed: u64) -> RunReport {
    let kernel = Kernel::new();
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut threads = Vec::new();
    for _ in 0..12 {
        let spec = match lcg(&mut state) % 3 {
            0 => CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(2)),
            1 => CadenceSpec::new(CadenceClass::Latency, 512 + (lcg(&mut state) % 2048) as u32)
                .with_latency(millis(1)),
            _ => CadenceSpec::new(
                CadenceClass::Throughput,
                256 + (lcg(&mut state) % 2048) as u32,
            ),
        };
        threads.push(
            kernel
                .spawn_task(HandleTable::with_default_quota(), spec)
                .expect("任务"),
        );
    }

    let mut simulator = Simulator::new(kernel, seed)
        .with_cpus(2)
        .with_quantum(10)
        .with_step_limit(10_000);
    for thread in threads {
        let limit = 1 + (lcg(&mut state) % 6);
        let runs = Arc::new(AtomicU64::new(0));
        simulator.register(
            thread,
            Box::new(move |_, _| {
                if runs.fetch_add(1, Ordering::Relaxed) >= limit {
                    ThreadStep::Exit
                } else {
                    ThreadStep::Continue
                }
            }),
        );
    }
    simulator.run().expect("随机压力回放完成")
}

#[test]
fn replays_randomized_workload_deterministically() {
    let first = stress_replay(1234);
    let second = stress_replay(1234);
    assert_eq!(first, second, "相同种子应产生相同的压力回放");
    assert_eq!(first.exits, 12);

    let other = stress_replay(4321);
    assert_eq!(other.exits, 12);
    assert_ne!(first.order, other.order, "不同种子应产生不同调度序列");
}

#[test]
fn accepts_two_to_one_share_within_two_percent() {
    let kernel = Kernel::new();
    let heavy = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Throughput, 2048),
        )
        .expect("高权重任务");
    let light = kernel
        .spawn_task(
            HandleTable::with_default_quota(),
            CadenceSpec::new(CadenceClass::Throughput, 1024),
        )
        .expect("低权重任务");

    const QUANTUM: u64 = 1_000;
    const WINDOW: u64 = 1_000_000;
    let consumed = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
    let mut simulator = Simulator::new(kernel, 77)
        .with_quantum(QUANTUM)
        .with_step_limit(10_000);
    for (thread, slot) in [(heavy, 0), (light, 1)] {
        let consumed = consumed.clone();
        simulator.register(
            thread,
            Box::new(move |kernel, tid| {
                if let Some(meter) = kernel.task_meter_snapshot(tid) {
                    consumed[slot].store(meter.consumed, Ordering::Relaxed);
                }
                if kernel.now() >= WINDOW {
                    ThreadStep::Exit
                } else {
                    ThreadStep::Continue
                }
            }),
        );
    }

    let report = simulator.run().expect("份额验收回放完成");
    assert_eq!(report.exits, 2);
    let heavy_consumed = consumed[0].load(Ordering::Relaxed);
    let light_consumed = consumed[1].load(Ordering::Relaxed);
    assert!(heavy_consumed > 0 && light_consumed > 0);
    // 1 秒窗口内 2:1 份额，误差 ≤ 2%。
    assert!(
        heavy_consumed * 100 >= light_consumed * 196
            && heavy_consumed * 100 <= light_consumed * 204,
        "2:1 份额误差超过 2%：{heavy_consumed}:{light_consumed}"
    );
}

#[test]
fn accepts_migration_on_four_cpus() {
    let kernel = Kernel::new();
    let mut threads = Vec::new();
    for _ in 0..8 {
        threads.push(
            kernel
                .spawn_task(
                    HandleTable::with_default_quota(),
                    CadenceSpec::new(CadenceClass::Throughput, 1024),
                )
                .expect("任务"),
        );
    }

    let migrations = Arc::new(AtomicU64::new(0));
    let mut simulator = Simulator::new(kernel, 99)
        .with_cpus(4)
        .with_quantum(50)
        .with_step_limit(5_000);
    for thread in threads {
        let runs = Arc::new(AtomicU64::new(0));
        let observed = migrations.clone();
        simulator.register(
            thread,
            Box::new(move |kernel, tid| {
                if let Some(meter) = kernel.task_meter_snapshot(tid) {
                    observed.fetch_max(meter.migrations, Ordering::Relaxed);
                }
                if runs.fetch_add(1, Ordering::Relaxed) >= 4 {
                    ThreadStep::Exit
                } else {
                    ThreadStep::Continue
                }
            }),
        );
    }

    let report = simulator.run().expect("四核迁移回放完成");
    assert_eq!(report.exits, 8);
    assert!(
        migrations.load(Ordering::Relaxed) > 0,
        "四核回放应发生跨核迁移"
    );
}

#[test]
fn reports_kernel_state_after_run() {
    let kernel = Kernel::new();
    let worker = kernel
        .spawn_task(HandleTable::new(8), DEFAULT_SPEC)
        .expect("创建任务");

    let mut simulator = Simulator::new(kernel, 3);
    simulator.register(worker, Box::new(|_, _| ThreadStep::Exit));
    simulator.run().expect("运行完成");

    let report = simulator.dump();
    assert!(report.contains("kernel: threads=0"), "{report}");
    assert!(simulator.threads().is_empty());
}
