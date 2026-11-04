//! QEMU 冒烟测试：启动内核并检查串口输出。

mod qmp;

use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use qmp::inject;

use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

/// 等待内核退出的上限。
const TIMEOUT: Duration = Duration::from_secs(30);

/// `isa-debug-exit` 的成功退出码：`(0 << 1) | 1`。
const QEMU_SUCCESS: i32 = 1;

/// 异常测试的退出码：`(70 << 1) | 1`。
const QEMU_EXCEPTION: i32 = 141;

/// 冒烟模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SmokeMode {
    /// 正常启动并校验 BootInfo
    #[value(name = "boot")]
    Boot,
    /// 非法指令异常
    #[value(name = "ud2")]
    Ud2,
    /// 空指针写入触发页错误
    #[value(name = "page")]
    Page,
    /// 本地 APIC 定时器中断
    #[value(name = "timer")]
    Timer,
    /// 帧缓冲文本输出
    #[value(name = "fb")]
    Framebuffer,
    /// HolyShell 脚本化命令
    #[value(name = "shell")]
    Shell,
    /// 键盘输入驱动的交互式 HolyShell
    #[value(name = "input")]
    Input,
    /// 定时器抢占调度
    #[value(name = "preempt")]
    Preempt,
    /// 多 CPU MADT 本地 APIC 枚举
    #[value(name = "smp")]
    Smp,
}

impl SmokeMode {
    fn cmdline(self) -> Option<&'static str> {
        match self {
            Self::Boot => None,
            Self::Ud2 => Some("fault-test"),
            Self::Page => Some("page-fault-test"),
            Self::Timer => Some("timer-test"),
            Self::Framebuffer => Some("fb-test"),
            Self::Shell => Some(
                "shell-test help;focus hk-server-echo;tasks;start hk-server-device;tasks;handles;services;restart hk-server-demo;services;mounts;start hk-server-fault;start hk-server-fork;start hk-server-latency;tasks;trace;stats",
            ),
            Self::Input => Some("shell-interactive"),
            Self::Preempt => None,
            Self::Smp => Some("shell-test tasks;stats"),
        }
    }

    fn expected(self) -> &'static [&'static str] {
        match self {
            Self::Boot => &[
                "TSC 频率",
                "启动信息校验通过",
                "内核初始化耗时",
                "加载服务并进入用户态",
                "hk-server-demo running",
                "hk-server-echo 收到请求 21",
                "hk-server-demo 收到回复 42",
                "hk-server-demo arena ok",
                "hk-server-echo 收到句柄",
                "hk-server-demo 句柄转移 ok",
                "hk-server-echo 已发出通知",
                "hk-server-demo 通知等待 ok",
                "hk-server-device: 发现",
                "hk-server-spin: done",
                "hk-server-spin: closed",
                "调度 tick：",
                "启动耗时",
                "服务进入空闲",
            ],
            Self::Ud2 => &["内核异常：非法指令"],
            Self::Page => &["内核异常：页错误"],
            Self::Timer => &["定时器中断 5 次"],
            Self::Framebuffer => &["帧缓冲控制台: 80x36 字符"],
            Self::Shell => &[
                "HolyShell",
                "当前任务：hk-server-echo",
                "已启动：hk-server-device",
                "任务表（5 项）",
                "父",
                "句柄",
                "Arena",
                "VSpace",
                "4/64K",
                "Handle 计数",
                "重启",
                "已重启：hk-server-demo",
                "hk-server-demo running",
                "hk-server-demo 收到回复 42",
                "hk-server-demo 句柄转移 ok",
                "hk-server-demo 通知等待 ok",
                "用户异常：页错误",
                "hk-server-fault",
                "hk-server-fork: child 22",
                "hk-server-fork: parent 33",
                "hk-server-latency: avg",
                "trace（",
                "handle",
                "上下文切换：",
                "IPC call/reply：",
                "hk-server-demo",
                "hk-server-echo",
                "hk-server-device",
                "hk-server-spin",
                "尚无挂载",
            ],
            Self::Input => &[
                "IOAPIC 就绪",
                "HolyShell 交互模式",
                "hk> echo hello-ioapic",
                "hello-ioapic",
                "求值：boot.hsh",
                "脚本：boot.hsh",
            ],
            Self::Preempt => &["hk-server-demo 收到回复 42", "hk-server-spin: done"],
            Self::Smp => &[
                "本地 APIC：4 个",
                "AP 在线：3 个（启动 3，IPI 1 次）",
                "CPU 调度：",
                "CPU1=",
            ],
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Boot
            | Self::Timer
            | Self::Framebuffer
            | Self::Shell
            | Self::Input
            | Self::Preempt => QEMU_SUCCESS,
            Self::Smp => QEMU_SUCCESS,
            Self::Ud2 | Self::Page => QEMU_EXCEPTION,
        }
    }

    fn log_name(self) -> &'static str {
        match self {
            Self::Boot => "build/serial.log",
            Self::Ud2 => "build/serial-fault.log",
            Self::Page => "build/serial-page-fault.log",
            Self::Timer => "build/serial-timer.log",
            Self::Framebuffer => "build/serial-fb.log",
            Self::Shell => "build/serial-shell.log",
            Self::Input => "build/serial-input.log",
            Self::Preempt => "build/serial-preempt.log",
            Self::Smp => "build/serial-smp.log",
        }
    }

    /// 需要断言先后的输出片段。
    fn ordered(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Preempt => Some(("hk-server-demo 收到回复 42", "hk-server-spin: done")),
            _ => None,
        }
    }
}

/// 启动内核并校验串口输出。
pub fn run(selection: Selection, release: bool, mode: SmokeMode) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 || targets[0] != Target::X86_64Qemu {
        return Err(Failure::new("smoke 只支持 x86_64-qemu"));
    }

    let iso = crate::image::run(selection, release, mode.cmdline())?;
    let log = workspace::root().join(mode.log_name());
    let _ = fs::remove_file(&log);

    let socket = workspace::root().join("build/qmp.sock");
    if mode == SmokeMode::Input {
        let _ = fs::remove_file(&socket);
    }

    let mut command = Command::new("qemu-system-x86_64");
    command
        .args(["-M", "q35", "-m", "256M", "-cdrom"])
        .arg(&iso)
        .args(["-boot", "d", "-display", "none", "-no-reboot", "-serial"])
        .arg(format!("file:{}", log.display()))
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
    if mode == SmokeMode::Smp {
        command.args(["-smp", "4"]);
    }
    if mode == SmokeMode::Input {
        command
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", socket.display()));
    }

    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;

    if mode == SmokeMode::Input {
        thread::sleep(Duration::from_secs(3));
        inject(&socket, "echo hello-ioapic\n")?;
        thread::sleep(Duration::from_millis(500));
        inject(&socket, "use boot.hsh\n")?;
        thread::sleep(Duration::from_millis(500));
        inject(&socket, "exit\n")?;
    }

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(Failure::new(format!("等待 QEMU 失败：{error}"))),
        }
    };

    let expected = mode.expected();
    let read_deadline = Instant::now() + Duration::from_secs(1);
    let text = loop {
        let bytes = fs::read(&log).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if expected.iter().all(|item| text.contains(item)) || Instant::now() > read_deadline {
            break text;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let expected_code = mode.exit_code();
    match status {
        Some(status) if status.code() == Some(expected_code) => {}
        Some(status) => {
            return Err(Failure::new(format!(
                "QEMU 退出码 {:?}，期望 {expected_code}\n串口输出：\n{text}",
                status.code()
            )));
        }
        None => {
            return Err(Failure::new(format!("QEMU 超时未退出\n串口输出：\n{text}")));
        }
    }

    for expected in mode.expected() {
        if !text.contains(expected) {
            return Err(Failure::new(format!(
                "串口输出缺少「{expected}」\n串口输出：\n{text}"
            )));
        }
    }

    if let Some((first, second)) = mode.ordered() {
        let in_order = matches!(
            (text.find(first), text.find(second)),
            (Some(first_at), Some(second_at)) if first_at < second_at
        );
        if !in_order {
            return Err(Failure::new(format!(
                "串口输出顺序不符：「{first}」应在「{second}」之前\n串口输出：\n{text}"
            )));
        }
    }
    Ok(())
}
