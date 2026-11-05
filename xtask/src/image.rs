//! 构建可启动镜像。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

/// Limine 安装目录的默认位置。
const DEFAULT_LIMINE_DIR: &str = "/usr/share/limine";

const LIMINE_CONFIG: &str = "timeout: 0\nserial: yes\n\n/HolyKernel\n    protocol: limine\n    kernel_path: boot():/boot/hk-x86_64\n    module_path: boot():/boot/hk-server-demo\n    module_path: boot():/boot/hk-server-echo\n    module_path: boot():/boot/hk-server-device\n    module_path: boot():/boot/hk-server-spin\n    module_path: boot():/boot/hk-server-virtio\n    module_path: boot():/boot/hk-server-vfs\n    module_path: boot():/boot/hk-server-fault\n    module_path: boot():/boot/hk-server-fork\n    module_path: boot():/boot/hk-server-latency\n    module_path: boot():/boot/hk-server-block\n    module_path: boot():/boot/hk-server-virtio-net\n    module_path: boot():/boot/hk-server-net\n    module_path: boot():/boot/hk-server-netstat\n    module_path: boot():/boot/hk-server-netsock\n    module_path: boot():/boot/hk-server-console\n    module_path: boot():/boot/hk-server-console-test\n    module_path: boot():/boot/hk-server-display\n";

/// 随镜像打包的 HolyShell 脚本。
const SCRIPT_MODULES: &[&str] = &["boot.hsh", "services.hsh"];

fn limine_config(cmdline: Option<&str>) -> String {
    let mut config = String::from(LIMINE_CONFIG);
    for script in SCRIPT_MODULES {
        config.push_str(&format!("    module_path: boot():/boot/scripts/{script}\n"));
    }
    if let Some(cmdline) = cmdline {
        config.push_str(&format!("    cmdline: {cmdline}\n"));
    }
    config
}

/// 构建 x86_64 QEMU 启动镜像，返回 ISO 路径。
pub fn run(selection: Selection, release: bool, cmdline: Option<&str>) -> Result<PathBuf> {
    let targets = selection.targets();
    if targets.len() != 1 || targets[0] != Target::X86_64Qemu {
        return Err(Failure::new("image 只支持 x86_64-qemu"));
    }

    build_kernel(release)?;
    build_iso(release, cmdline)
}

fn build_kernel(release: bool) -> Result<()> {
    let mut command = cargo::command();
    command.args([
        "build",
        "--target",
        "x86_64-unknown-none",
        "-p",
        "hk-x86_64",
        "-p",
        "hk-server-demo",
        "-p",
        "hk-server-echo",
        "-p",
        "hk-server-device",
        "-p",
        "hk-server-spin",
        "-p",
        "hk-server-virtio",
        "-p",
        "hk-server-vfs",
        "-p",
        "hk-server-fault",
        "-p",
        "hk-server-fork",
        "-p",
        "hk-server-latency",
        "-p",
        "hk-server-block",
        "-p",
        "hk-server-virtio-net",
        "-p",
        "hk-server-net",
        "-p",
        "hk-server-netstat",
        "-p",
        "hk-server-netsock",
        "-p",
        "hk-server-console",
        "-p",
        "hk-server-console-test",
        "-p",
        "hk-server-display",
    ]);
    if release {
        command.arg("--release");
    }
    cargo::run(command)
}

fn binary_path(name: &str, release: bool) -> PathBuf {
    workspace::root()
        .join("target/x86_64-unknown-none")
        .join(if release { "release" } else { "debug" })
        .join(name)
}

fn limine_dir() -> PathBuf {
    std::env::var_os("HK_LIMINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LIMINE_DIR))
}

fn build_iso(release: bool, cmdline: Option<&str>) -> Result<PathBuf> {
    let root = workspace::root();
    let staging = root.join("build/iso");
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| Failure::new(format!("清理 {} 失败：{error}", staging.display())))?;
    }

    let boot = staging.join("boot");
    let efi = staging.join("EFI/BOOT");
    let scripts = boot.join("scripts");
    create_dir(&boot)?;
    create_dir(&efi)?;
    create_dir(&scripts)?;

    let limine = limine_dir();
    copy(&binary_path("hk-x86_64", release), &boot.join("hk-x86_64"))?;
    copy(
        &binary_path("hk-server-demo", release),
        &boot.join("hk-server-demo"),
    )?;
    copy(
        &binary_path("hk-server-echo", release),
        &boot.join("hk-server-echo"),
    )?;
    copy(
        &binary_path("hk-server-device", release),
        &boot.join("hk-server-device"),
    )?;
    copy(
        &binary_path("hk-server-spin", release),
        &boot.join("hk-server-spin"),
    )?;
    copy(
        &binary_path("hk-server-virtio", release),
        &boot.join("hk-server-virtio"),
    )?;
    copy(
        &binary_path("hk-server-vfs", release),
        &boot.join("hk-server-vfs"),
    )?;
    copy(
        &binary_path("hk-server-fault", release),
        &boot.join("hk-server-fault"),
    )?;
    copy(
        &binary_path("hk-server-fork", release),
        &boot.join("hk-server-fork"),
    )?;
    copy(
        &binary_path("hk-server-latency", release),
        &boot.join("hk-server-latency"),
    )?;
    copy(
        &binary_path("hk-server-block", release),
        &boot.join("hk-server-block"),
    )?;
    copy(
        &binary_path("hk-server-virtio-net", release),
        &boot.join("hk-server-virtio-net"),
    )?;
    copy(
        &binary_path("hk-server-net", release),
        &boot.join("hk-server-net"),
    )?;
    copy(
        &binary_path("hk-server-netstat", release),
        &boot.join("hk-server-netstat"),
    )?;
    copy(
        &binary_path("hk-server-netsock", release),
        &boot.join("hk-server-netsock"),
    )?;
    copy(
        &binary_path("hk-server-console", release),
        &boot.join("hk-server-console"),
    )?;
    copy(
        &binary_path("hk-server-console-test", release),
        &boot.join("hk-server-console-test"),
    )?;
    copy(
        &binary_path("hk-server-display", release),
        &boot.join("hk-server-display"),
    )?;
    for script in SCRIPT_MODULES {
        copy(&root.join("scripts").join(script), &scripts.join(script))?;
    }
    copy(
        &limine.join("limine-bios-cd.bin"),
        &boot.join("limine-bios-cd.bin"),
    )?;
    copy(
        &limine.join("limine-bios.sys"),
        &boot.join("limine-bios.sys"),
    )?;
    copy(
        &limine.join("limine-uefi-cd.bin"),
        &boot.join("limine-uefi-cd.bin"),
    )?;
    copy(&limine.join("BOOTX64.EFI"), &efi.join("BOOTX64.EFI"))?;
    fs::write(boot.join("limine.conf"), limine_config(cmdline))
        .map_err(|error| Failure::new(format!("写入 limine.conf 失败：{error}")))?;

    let iso = root.join("build/holy-x86_64.iso");
    let status = Command::new("xorriso")
        .current_dir(&staging)
        .args([
            "-as",
            "mkisofs",
            "-b",
            "boot/limine-bios-cd.bin",
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
            "--efi-boot",
            "boot/limine-uefi-cd.bin",
            "-efi-boot-part",
            "--efi-boot-image",
            "--protective-msdos-label",
            ".",
            "-o",
        ])
        .arg(&iso)
        .status()
        .map_err(|error| Failure::new(format!("启动 xorriso 失败：{error}")))?;
    if !status.success() {
        return Err(Failure::new(format!("xorriso 失败：{status}")));
    }

    let status = Command::new("limine")
        .arg("bios-install")
        .arg(&iso)
        .status()
        .map_err(|error| Failure::new(format!("启动 limine 失败：{error}")))?;
    if !status.success() {
        return Err(Failure::new(format!("limine bios-install 失败：{status}")));
    }

    Ok(iso)
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", path.display())))
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).map_err(|error| {
        Failure::new(format!(
            "复制 {} 到 {} 失败：{error}",
            from.display(),
            to.display()
        ))
    })?;
    Ok(())
}
