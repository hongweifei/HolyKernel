//! 仿真器测试。

use alloc::boxed::Box;

use hk_handle::{HandleTable, Rights};
use hk_ipc::Message;
use hk_kernel::{Kernel, Priority};

use crate::{RunReport, SimError, Simulator, ThreadStep};

#[test]
fn runs_send_and_recv_to_completion() {
    let kernel = Kernel::new();
    let server = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
    let client = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);

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
    let server = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
    let client = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);

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
    let worker = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
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
    let worker = kernel.spawn_thread(HandleTable::new(8), Priority::NORMAL);

    let mut simulator = Simulator::new(kernel, 1).with_step_limit(5);
    simulator.register(worker, Box::new(|_, _| ThreadStep::Continue));

    assert_eq!(simulator.run(), Err(SimError::StepLimit { steps: 6 }));
}

#[test]
#[should_panic(expected = "线程缺少执行体")]
fn panics_on_thread_without_body() {
    let kernel = Kernel::new();
    kernel.spawn_thread(HandleTable::new(8), Priority::NORMAL);

    let mut simulator = Simulator::new(kernel, 1);
    let _ = simulator.run();
}

#[test]
fn reports_kernel_state_after_run() {
    let kernel = Kernel::new();
    let worker = kernel.spawn_thread(HandleTable::new(8), Priority::NORMAL);

    let mut simulator = Simulator::new(kernel, 3);
    simulator.register(worker, Box::new(|_, _| ThreadStep::Exit));
    simulator.run().expect("运行完成");

    let report = simulator.dump();
    assert!(report.contains("kernel: threads=0"), "{report}");
    assert!(simulator.threads().is_empty());
}
