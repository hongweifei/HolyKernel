//! 在 QEMU 中运行镜像。

use std::path::Path;
use std::process::Command;

use crate::failure::{Failure, Result};

/// `isa-debug-exit` 的成功退出码：`(0 << 1) | 1`。
const QEMU_SUCCESS: i32 = 1;

/// 启动 QEMU 并等待内核退出。
pub fn run(iso: &Path) -> Result<()> {
    let status = Command::new("qemu-system-x86_64")
        .args(["-M", "q35", "-m", "256M", "-cdrom"])
        .arg(iso)
        .args([
            "-boot",
            "d",
            "-serial",
            "stdio",
            "-display",
            "none",
            "-no-reboot",
            "-device",
            "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ])
        .status()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;

    match status.code() {
        Some(QEMU_SUCCESS) => Ok(()),
        Some(code) => Err(Failure::new(format!("QEMU 退出码 {code}"))),
        None => Err(Failure::new("QEMU 被信号终止")),
    }
}
