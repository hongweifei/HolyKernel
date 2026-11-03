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

const LIMINE_CONFIG: &str = "timeout: 0\nserial: yes\n\n/HolyKernel\n    protocol: limine\n    kernel_path: boot():/boot/hk-x86_64\n    module_path: boot():/boot/hk-server-demo\n";

fn limine_config(cmdline: Option<&str>) -> String {
    let mut config = String::from(LIMINE_CONFIG);
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
    create_dir(&boot)?;
    create_dir(&efi)?;

    let limine = limine_dir();
    copy(&binary_path("hk-x86_64", release), &boot.join("hk-x86_64"))?;
    copy(
        &binary_path("hk-server-demo", release),
        &boot.join("hk-server-demo"),
    )?;
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
