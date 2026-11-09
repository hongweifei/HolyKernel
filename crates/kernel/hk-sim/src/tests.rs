//! 仿真器测试。

use alloc::boxed::Box;
use alloc::sync::Arc;
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
