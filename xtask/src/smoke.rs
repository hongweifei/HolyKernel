//! QEMU 冒烟测试：启动内核并检查串口输出。

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
                "shell-test help;focus hk-server-echo;tasks;start hk-server-device;tasks;services;mounts",
            ),
            Self::Input => Some("shell-interactive"),
            Self::Preempt => None,
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
                "hk-server-device: 发现",
                "hk-server-spin: done",
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
            ],
            Self::Preempt => &["hk-server-demo 收到回复 42", "hk-server-spin: done"],
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

    let text = fs::read_to_string(&log).unwrap_or_default();
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

#[cfg(unix)]
fn inject(socket: &Path, text: &str) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(Failure::new(format!("连接 QMP 失败：{error}"))),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| Failure::new(format!("设置 QMP 超时失败：{error}")))?;

    let mut writer = stream
        .try_clone()
        .map_err(|error| Failure::new(format!("复制 QMP 连接失败：{error}")))?;
    let mut reader = BufReader::new(stream);
    let mut greeting = String::new();
    reader
        .read_line(&mut greeting)
        .map_err(|error| Failure::new(format!("读取 QMP 问候失败：{error}")))?;

    writer
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .and_then(|()| writer.flush())
        .map_err(|error| Failure::new(format!("发送 QMP 握手失败：{error}")))?;
    thread::sleep(Duration::from_millis(100));

    for key in qcodes(text) {
        let command = format!(
            "{{\"execute\":\"send-key\",\"arguments\":{{\"keys\":[{{\"type\":\"qcode\",\"data\":\"{key}\"}}]}}}}\n"
        );
        writer
            .write_all(command.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|error| Failure::new(format!("发送按键失败：{error}")))?;
        thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}

#[cfg(not(unix))]
fn inject(_socket: &Path, _text: &str) -> Result<()> {
    Err(Failure::new("键盘输入冒烟只在 Unix 主机上运行"))
}

fn qcodes(text: &str) -> Vec<String> {
    text.chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' => character.to_string(),
            ' ' => String::from("spc"),
            '-' => String::from("minus"),
            '\n' | '\r' => String::from("ret"),
            _ => String::from("spc"),
        })
        .collect()
}
