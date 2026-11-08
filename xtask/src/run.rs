//! 运行目标。

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::qemu::Machine;
use crate::target::{Selection, Target};

/// 运行选中的目标。
///
/// Hosted 直接启动进程；x86_64 QEMU 先构建镜像再启动虚拟机；AArch64 的启动镜像
/// 在 M8 提供。`machine` 控制 QEMU 的内存与 CPU 数量。
pub fn run(
    selection: Selection,
    release: bool,
    arguments: &[String],
    ui: bool,
    machine: Machine,
) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 {
        return Err(Failure::new("run 只接受单个目标"));
    }

    match targets[0] {
        Target::Hosted => launch_hosted(release, arguments),
        Target::X86_64Qemu => {
            if !arguments.is_empty() {
                return Err(Failure::new("x86_64-qemu 不接受额外参数"));
            }
            // 图形桌面使用 H: 卷：文档、Welcome 与 Help 都住在 HolyFS 上。
            let cmdline = if ui {
                "desktop vfs.fs=holyfs"
            } else {
                "shell-interactive"
            };
            let iso = crate::image::run(selection, release, Some(cmdline))?;
            if ui {
                let disk = crate::workspace::root().join("build/run-holy.img");
                crate::disk::ensure_holyfs(&disk)?;
                crate::qemu::run(&iso, &disk, ui, &machine)
            } else {
                let disk = crate::workspace::root().join("build/run-disk.img");
                crate::disk::ensure_ext4(&disk)?;
                crate::qemu::run(&iso, &disk, ui, &machine)
            }
        }
        Target::Aarch64Qemu => Err(Failure::new(
            "aarch64-qemu 的启动镜像在 M8 提供；当前使用 cargo xtask build --target aarch64-qemu 校验构建",
        )),
    }
}

fn launch_hosted(release: bool, arguments: &[String]) -> Result<()> {
    let mut command = cargo::locked();
    command.args(["run", "-p", "hk-hosted"]);
    if release {
        command.arg("--release");
    }
    if !arguments.is_empty() {
        command.arg("--");
        command.args(arguments);
    }
    cargo::run(command)
}
