//! 运行目标。

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::qemu::Machine;
use crate::target::{Selection, Target};

/// 运行选中的目标。
///
/// Hosted 直接启动进程；x86_64 QEMU 先构建镜像再启动虚拟机；AArch64 的启动镜像
/// 在 M8 提供。`machine` 控制 QEMU 的内存与 CPU 数量；`gpu` 让图形桌面接管
/// virtio-gpu 扫描输出。
pub fn run(
    selection: Selection,
    release: bool,
    arguments: &[String],
    ui: bool,
    gpu: bool,
    machine: Machine,
) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 {
        return Err(Failure::new("run 只接受单个目标"));
    }

    match targets[0] {
        Target::Hosted => {
            if gpu {
                return Err(Failure::new("--gpu 只适用于 x86_64-qemu 图形桌面"));
            }
            launch_hosted(release, arguments)
        }
        Target::X86_64Qemu => {
            if !arguments.is_empty() {
                return Err(Failure::new("x86_64-qemu 不接受额外参数"));
            }
            let cmdline = boot_cmdline(ui, gpu)?;
            let iso = crate::image::run(selection, release, Some(cmdline))?;
            if ui {
                let disk = crate::workspace::root().join("build/run-holy.img");
                crate::disk::ensure_holyfs(&disk)?;
                crate::qemu::run(&iso, &disk, ui, gpu, &machine)
            } else {
                let disk = crate::workspace::root().join("build/run-disk.img");
                crate::disk::ensure_ext4(&disk)?;
                crate::qemu::run(&iso, &disk, ui, gpu, &machine)
            }
        }
        Target::Aarch64Qemu => Err(Failure::new(
            "aarch64-qemu 的启动镜像在 M8 提供；当前使用 cargo xtask build --target aarch64-qemu 校验构建",
        )),
    }
}

/// 启动命令行：图形桌面使用 H: 卷；`gpu` 让 Display 接管 virtio-gpu 扫描输出。
///
/// GPU 模式显式给出 1280x800：QEMU 交互窗口下设备默认模式常是 640x480，
/// 与线性帧缓冲桌面不一致。
fn boot_cmdline(ui: bool, gpu: bool) -> Result<&'static str> {
    match (ui, gpu) {
        (true, true) => Ok("desktop vfs.fs=holyfs gpu.scanout gpu.mode=1280x800"),
        (true, false) => Ok("desktop vfs.fs=holyfs"),
        (false, false) => Ok("shell-interactive"),
        (false, true) => Err(Failure::new("--gpu 需要与 --ui 一起使用")),
    }
}

fn launch_hosted(release: bool, arguments: &[String]) -> Result<()> {
    let mut command = cargo::locked();
    command.args(["run", "-p", "hk-image-hosted"]);
    if release {
        command.arg("--release");
    }
    if !arguments.is_empty() {
        command.arg("--");
        command.args(arguments);
    }
    cargo::run(command)
}

#[cfg(test)]
mod tests {
    use super::boot_cmdline;

    #[test]
    fn gpu_desktop_requests_scanout() {
        assert_eq!(
            boot_cmdline(true, true).expect("gpu 桌面"),
            "desktop vfs.fs=holyfs gpu.scanout gpu.mode=1280x800"
        );
        assert_eq!(
            boot_cmdline(true, false).expect("默认桌面"),
            "desktop vfs.fs=holyfs"
        );
        assert_eq!(
            boot_cmdline(false, false).expect("shell"),
            "shell-interactive"
        );
    }

    #[test]
    fn gpu_requires_ui() {
        assert!(boot_cmdline(false, true).is_err());
    }
}
