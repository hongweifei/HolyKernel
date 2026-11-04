//! Hosted 微内核演示：Root Server 启动两个服务，客户端完成两轮 call/reply。
//!
//! 服务通过 Endpoint 接收请求并回复；服务退出时置位完成通知，Root Server 等待
//! 两位全部到齐后退出。

use std::fmt::Write;

use hk_handle::{HandleTable, Rights};
use hk_ipc::Message;
use hk_kernel::{Kernel, Priority};
use hk_root_server::{RootServer, ServiceSpec};
use hk_sim::{RunReport, Simulator, ThreadStep};

mod cow;

/// 演示结果。
pub struct DemoReport {
    /// 服务名称与服务线程编号。
    pub services: Vec<(&'static str, u64)>,
    /// 仿真运行报告。
    pub run: RunReport,
    /// 结束后的内核状态报告。
    pub dump: String,
    /// 客户端拿到的时间值。
    pub clock_value: u64,
    /// COW fork 原型报告。
    pub cow: String,
}

/// 运行演示。
pub fn run(seed: u64) -> Result<DemoReport, String> {
    let kernel = Kernel::new();
    let root_server = RootServer::boot(
        &kernel,
        &[ServiceSpec::new("clock"), ServiceSpec::new("log")],
    )
    .map_err(|error| format!("启动 Root Server 失败：{error}"))?;

    let clock = *root_server
        .service("clock")
        .ok_or_else(|| String::from("服务清单缺少 clock"))?;
    let log = *root_server
        .service("log")
        .ok_or_else(|| String::from("服务清单缺少 log"))?;

    let client = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
    let clock_client = kernel
        .grant(client, clock.endpoint, Rights::READ | Rights::WRITE)
        .map_err(|error| format!("授予 clock 句柄失败：{error}"))?;
    let log_client = kernel
        .grant(client, log.endpoint, Rights::READ | Rights::WRITE)
        .map_err(|error| format!("授予 log 句柄失败：{error}"))?;

    let completion = kernel
        .create_notification(root_server.root(), Rights::READ | Rights::WRITE)
        .map_err(|error| format!("创建完成通知失败：{error}"))?;
    let completion_id = kernel
        .handle_id(root_server.root(), completion)
        .map_err(|error| format!("解析完成通知失败：{error}"))?;
    let clock_done = kernel
        .grant(clock.tcb, completion_id, Rights::WRITE)
        .map_err(|error| format!("授予 clock 完成通知失败：{error}"))?;
    let log_done = kernel
        .grant(log.tcb, completion_id, Rights::WRITE)
        .map_err(|error| format!("授予 log 完成通知失败：{error}"))?;

    let clock_value = 21;
    let mut simulator = Simulator::new(kernel, seed);

    let mut seen = 0_u64;
    simulator.register(
        root_server.root(),
        Box::new(
            move |kernel, tid| match kernel.wait(tid, completion, 0b11) {
                Ok(Some(bits)) => {
                    seen |= bits;
                    if seen == 0b11 {
                        ThreadStep::Exit
                    } else {
                        ThreadStep::Continue
                    }
                }
                Ok(None) => ThreadStep::Continue,
                Err(error) => panic!("Root Server 等待完成通知失败：{error}"),
            },
        ),
    );

    simulator.register(
        clock.tcb,
        Box::new(move |kernel, tid| match kernel.recv(tid, clock.handle) {
            Ok(Some(received)) => {
                let token = received.reply.expect("clock 收到调用");
                let value = received.message.words[0];
                kernel
                    .reply(
                        tid,
                        token,
                        Message::new(7).with_words([value * 2, 0, 0, 0, 0, 0, 0, 0]),
                    )
                    .expect("clock 回复");
                kernel
                    .signal(tid, clock_done, 0b01)
                    .expect("clock 置位完成通知");
                ThreadStep::Exit
            }
            Ok(None) => ThreadStep::Continue,
            Err(error) => panic!("clock 接收失败：{error}"),
        }),
    );

    simulator.register(
        log.tcb,
        Box::new(move |kernel, tid| match kernel.recv(tid, log.handle) {
            Ok(Some(received)) => {
                let token = received.reply.expect("log 收到调用");
                let label = received.message.label + 1;
                kernel
                    .reply(tid, token, Message::new(label))
                    .expect("log 回复");
                kernel
                    .signal(tid, log_done, 0b10)
                    .expect("log 置位完成通知");
                ThreadStep::Exit
            }
            Ok(None) => ThreadStep::Continue,
            Err(error) => panic!("log 接收失败：{error}"),
        }),
    );

    let mut stage = 0_u8;
    simulator.register(
        client,
        Box::new(move |kernel, tid| match stage {
            0 => {
                kernel
                    .call(
                        tid,
                        clock_client,
                        Message::new(1).with_words([clock_value, 0, 0, 0, 0, 0, 0, 0]),
                    )
                    .expect("调用 clock");
                stage = 1;
                ThreadStep::Continue
            }
            1 => {
                let reply = kernel.take_reply(tid).expect("clock 回复就绪");
                assert_eq!(reply.words[0], clock_value * 2, "clock 返回值不符");
                stage = 2;
                ThreadStep::Continue
            }
            2 => {
                kernel
                    .call(tid, log_client, Message::new(5))
                    .expect("调用 log");
                stage = 3;
                ThreadStep::Continue
            }
            3 => {
                let reply = kernel.take_reply(tid).expect("log 回复就绪");
                assert_eq!(reply.label, 6, "log 返回值不符");
                stage = 4;
                ThreadStep::Continue
            }
            _ => ThreadStep::Exit,
        }),
    );

    let run = simulator
        .run()
        .map_err(|error| format!("仿真失败：{error}"))?;
    let dump = simulator.dump();
    let cow = cow::run()?;

    let services = root_server
        .services()
        .iter()
        .map(|service| (service.name, service.tcb.get()))
        .collect();

    Ok(DemoReport {
        services,
        run,
        dump,
        clock_value: clock_value * 2,
        cow,
    })
}

/// 把演示结果格式化为报告。
pub fn describe(report: &DemoReport) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "内核仿真");
    let _ = writeln!(
        text,
        "  调度步数: {}  退出线程: {}  trace 事件: {}  种子: {}",
        report.run.steps, report.run.exits, report.run.trace_events, report.run.seed
    );
    let _ = writeln!(text, "  服务表:");
    for (name, tcb) in &report.services {
        let _ = writeln!(text, "    {name}  tcb={tcb}");
    }
    let _ = writeln!(text, "  客户端 clock 返回值: {}", report.clock_value);
    text.push_str(&report.cow);
    text.push_str(&report.dump);
    text
}
