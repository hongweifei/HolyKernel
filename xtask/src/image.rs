//! 构建可启动镜像。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use hk_image::FILE_SUFFIX;

use crate::cargo;
use crate::catalog::{SCRIPT_MODULES, SERVERS};
use crate::failure::{Failure, Result};
use crate::native::{SYMBOL_MODULE, generate_symbols, native_image};
use crate::target::{Selection, Target};
use crate::workspace;

/// Limine 安装目录的默认位置。
const DEFAULT_LIMINE_DIR: &str = "/usr/share/limine";

fn limine_config(cmdline: Option<&str>) -> String {
    let mut config = String::from(
        "timeout: 0\nserial: yes\n\n/HolyKernel\n    protocol: limine\n    kernel_path: boot():/boot/hk-x86_64\n",
    );
    for service in SERVERS {
        config.push_str(&format!(
            "    module_path: boot():/boot/{service}{FILE_SUFFIX}\n"
        ));
    }
    for script in SCRIPT_MODULES {
        config.push_str(&format!("    module_path: boot():/boot/scripts/{script}\n"));
    }
    config.push_str(&format!("    module_path: boot():/boot/{SYMBOL_MODULE}\n"));
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
    let symbols = generate_symbols(release)?;
    build_iso(release, cmdline, &symbols)
}

fn build_kernel(release: bool) -> Result<()> {
    let mut command = cargo::command();
    command.args([
        "build",
        "--target",
        "x86_64-unknown-none",
        "-p",
        "hk-x86_64",
    ]);
    for service in SERVERS {
        command.args(["-p", service]);
    }
    if release {
        command.arg("--release");
    }
    cargo::run(command)
}

pub(crate) fn binary_path(name: &str, release: bool) -> PathBuf {
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

fn build_iso(release: bool, cmdline: Option<&str>, symbols: &Path) -> Result<PathBuf> {
    let root = workspace::root();
    let suffix = std::env::var("HK_ISO_SUFFIX").unwrap_or_default();
    let staging = root.join(format!("build/iso{suffix}"));
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
    for service in SERVERS {
        let image = native_image(&binary_path(service, release))?;
        check_image_slot(service, &image)?;
        let target = boot.join(format!("{service}{FILE_SUFFIX}"));
        fs::write(&target, image)
            .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", target.display())))?;
    }
    for script in SCRIPT_MODULES {
        copy(&root.join("scripts").join(script), &scripts.join(script))?;
    }
    copy(symbols, &boot.join(SYMBOL_MODULE))?;
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

    let iso = root.join(format!("build/holy-x86_64{suffix}.iso"));
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

fn check_image_slot(service: &str, image: &[u8]) -> Result<()> {
    let base = hk_server_build::image_base(service).ok_or_else(|| {
        Failure::new(format!(
            "服务 {service} 未登记在 hk-server-build::SERVERS 中"
        ))
    })?;
    let slot_end = base + hk_server_build::IMAGE_STRIDE;
    let parsed = hk_image::Image::parse(image)
        .ok_or_else(|| Failure::new(format!("服务 {service} 原生镜像无效")))?;
    for region in parsed.regions() {
        let start = region.virtual_address;
        let end = start.saturating_add(region.memory_size);
        if start < base || end > slot_end {
            return Err(Failure::new(format!(
                "服务 {service} 超出镜像槽位：{start:#x}..{end:#x}，槽位 {base:#x}..{slot_end:#x}"
            )));
        }
    }
    Ok(())
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
