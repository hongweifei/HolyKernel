//! 构建可启动镜像。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use hk_image::FILE_SUFFIX;

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

    crate::build::build_target(Target::X86_64Qemu, release)?;
    let symbols = generate_symbols(release)?;
    build_iso(release, cmdline, &symbols)
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

/// 定位 `limine` 宿主工具：优先 `HK_LIMINE_DIR` 下的可执行文件，其次 PATH。
fn limine_tool() -> PathBuf {
    let dir = limine_dir();
    ["limine.exe", "limine"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("limine"))
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
        check_image_region(service, &image)?;
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
    let key_path = iso.with_extension("key");
    let key = staging_key(&staging)?;
    if iso.is_file()
        && fs::read_to_string(&key_path)
            .ok()
            .is_some_and(|cached| cached.trim() == key)
    {
        return Ok(iso);
    }
    let mut xorriso = Command::new("xorriso");
    // Windows 上的 xorriso 可能是 Cygwin 构建，只认 POSIX/相对路径，`D:\...` 会被
    // 当成 cwd 下的相对名。相对 staging 的写法在两个平台都成立。
    let iso_argument = Path::new("..").join(iso.file_name().expect("iso 有文件名"));
    xorriso
        .current_dir(&staging)
        .env("SOURCE_DATE_EPOCH", "0")
        .args([
            "-as",
            "mkisofs",
            "--modification-date=1970010100000000",
            "--set_all_file_dates",
            "1970010100000000",
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
        .arg(&iso_argument);
    let status = xorriso
        .status()
        .map_err(|error| Failure::new(format!("启动 xorriso 失败：{error}")))?;
    if !status.success() {
        return Err(Failure::new(format!("xorriso 失败：{status}")));
    }

    let status = Command::new(limine_tool())
        .arg("bios-install")
        .arg(&iso)
        .status()
        .map_err(|error| Failure::new(format!("启动 limine 失败：{error}")))?;
    if !status.success() {
        return Err(Failure::new(format!("limine bios-install 失败：{status}")));
    }
    fs::write(&key_path, format!("{key}\n"))
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", key_path.display())))?;

    Ok(iso)
}

fn check_image_region(service: &str, image: &[u8]) -> Result<()> {
    let base = hk_server_build::IMAGE_BASE;
    let limit = hk_server_build::IMAGE_LIMIT;
    let parsed = hk_image::Image::parse(image)
        .ok_or_else(|| Failure::new(format!("服务 {service} 原生镜像无效")))?;
    for region in parsed.regions() {
        let start = region.virtual_address;
        let end = start.saturating_add(region.memory_size);
        if start < base || end > limit {
            return Err(Failure::new(format!(
                "服务 {service} 超出共享镜像区：{start:#x}..{end:#x}，区间 {base:#x}..{limit:#x}"
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

/// 计算暂存树的内容键；内容相同则复用已生成的 ISO。
fn staging_key(directory: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(directory, &mut files)?;
    files.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in b"iso-format-v2" {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for file in files {
        let relative = file.strip_prefix(directory).unwrap_or(&file);
        for byte in relative.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let bytes = fs::read(&file)
            .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", file.display())))?;
        for byte in &bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(format!("{hash:016x}"))
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", directory.display())))?;
    for entry in entries {
        let path = entry
            .map_err(|error| Failure::new(format!("读取目录项失败：{error}")))?
            .path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}
