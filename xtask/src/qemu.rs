//! 在 QEMU 中运行镜像。

use std::path::Path;
use std::process::Command;

use crate::failure::{Failure, Result};

/// `isa-debug-exit` 的成功退出码：`(0 << 1) | 1`。
const QEMU_SUCCESS: i32 = 1;

/// 启动 QEMU 并等待内核退出。
pub fn run(iso: &Path, disk: &Path, ui: bool) -> Result<()> {
    let mut command = Command::new("qemu-system-x86_64");
    command
        .args(["-M", "q35", "-m", "256M", "-cdrom"])
        .arg(iso)
        .args(["-boot", "d", "-serial", "stdio", "-no-reboot"]);
    if Path::new("/dev/kvm").exists() {
        command.args(["-accel", "kvm", "-cpu", "host"]);
    }
    if !ui {
        command.args(["-display", "none"]);
    }
    command
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk,format=raw",
            disk.display()
        ))
        .args(["-device", "virtio-blk-pci,drive=disk"]);
    if ui {
        command.args(["-device", "virtio-keyboard-pci"]);
        command.args(["-device", "virtio-tablet-pci"]);
    }
    let status = command
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .status()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;

    match status.code() {
        Some(QEMU_SUCCESS) => Ok(()),
        Some(code) => Err(Failure::new(format!("QEMU 退出码 {code}"))),
        None => Err(Failure::new("QEMU 被信号终止")),
    }
}
