//! QEMU 冒烟测试：启动内核并检查串口输出。

use std::fs;
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

/// 异常测试类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum FaultKind {
    /// 非法指令
    #[value(name = "ud2")]
    Ud2,
    /// 空指针写入触发页错误
    #[value(name = "page")]
    Page,
}

impl FaultKind {
    fn cmdline(self) -> &'static str {
        match self {
            Self::Ud2 => "fault-test",
            Self::Page => "page-fault-test",
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Self::Ud2 => "内核异常：非法指令",
            Self::Page => "内核异常：页错误",
        }
    }

    fn log_name(self) -> &'static str {
        match self {
            Self::Ud2 => "build/serial-fault.log",
            Self::Page => "build/serial-page-fault.log",
        }
    }
}

/// 启动内核并校验串口输出。
///
/// `fault` 指定时通过内核命令行触发对应异常，检查异常现场输出。
pub fn run(selection: Selection, release: bool, fault: Option<FaultKind>) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 || targets[0] != Target::X86_64Qemu {
        return Err(Failure::new("smoke 只支持 x86_64-qemu"));
    }

    let iso = crate::image::run(selection, release, fault.map(FaultKind::cmdline))?;
    let log = workspace::root().join(match fault {
        Some(kind) => kind.log_name(),
        None => "build/serial.log",
    });
    let _ = fs::remove_file(&log);

    let mut child = Command::new("qemu-system-x86_64")
        .args(["-M", "q35", "-m", "256M", "-cdrom"])
        .arg(&iso)
        .args(["-boot", "d", "-display", "none", "-no-reboot", "-serial"])
        .arg(format!("file:{}", log.display()))
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;

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
    let expected_code = if fault.is_some() {
        QEMU_EXCEPTION
    } else {
        QEMU_SUCCESS
    };
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

    let expected = match fault {
        Some(kind) => kind.expected(),
        None => "启动信息校验通过",
    };
    if !text.contains(expected) {
        return Err(Failure::new(format!(
            "串口输出缺少「{expected}」\n串口输出：\n{text}"
        )));
    }
    Ok(())
}
