//! 依赖方向检查：内核不依赖实现，服务不依赖内核。

use std::fs;
use std::path::Path;

use crate::failure::{Failure, Result};
use crate::workspace;

/// 实现层 crate 禁止依赖的类别。
const IMPL_FORBIDDEN: &[&str] = &[
    "hk-runtime",
    "hk-root-server",
    "hk-platform-",
    "hk-server-",
    "hk-net",
    "hk-vfs",
    "hk-fs-",
    "hk-block",
    "hk-driver",
    "hk-framebuffer",
    "hk-x86_64",
    "hk-hosted",
    "hk-sim",
];

/// 核内库禁止依赖的类别。
const LIB_FORBIDDEN: &[&str] = &[
    "hk-kernel",
    "hk-runtime",
    "hk-root-server",
    "hk-platform-",
    "hk-server-",
    "hk-x86_64",
    "hk-hosted",
];

/// 用户态库禁止依赖的类别（允许使用 `hk-server-api`）。
const USERSPACE_LIB_FORBIDDEN: &[&str] = &[
    "hk-kernel",
    "hk-runtime",
    "hk-root-server",
    "hk-platform-",
    "hk-x86_64",
    "hk-hosted",
];

/// 文件系统契约禁止依赖的类别。
const CONTRACT_FORBIDDEN: &[&str] = &[
    "hk-kernel",
    "hk-runtime",
    "hk-root-server",
    "hk-platform-",
    "hk-x86_64",
    "hk-hosted",
    "hk-vfs",
    "hk-fs-",
    "hk-server-",
];

/// HolyFS 原生实现禁止依赖的类别：不得依赖 VFS 或其适配器。
const HOLYFS_IMPL_FORBIDDEN: &[&str] = &[
    "hk-kernel",
    "hk-runtime",
    "hk-root-server",
    "hk-platform-",
    "hk-x86_64",
    "hk-hosted",
    "hk-vfs",
    "hk-fs-holyfs-vfs",
];

/// 服务禁止依赖的类别。
const SERVER_FORBIDDEN: &[&str] = &["hk-kernel", "hk-runtime", "hk-root-server", "hk-platform-"];

/// 服务线协议层禁止依赖的类别：只允许内核面与 ABI 常量。
///
/// 服务二进制不是库 crate，无法被依赖，因此这里不列 `hk-server-*`。
const PROTO_FORBIDDEN: &[&str] = &[
    "hk-block",
    "hk-canvas",
    "hk-checkpoint",
    "hk-debug",
    "hk-driver",
    "hk-fs-",
    "hk-kernel",
    "hk-net",
    "hk-platform-",
    "hk-root-server",
    "hk-runtime",
    "hk-vfs",
    "hk-x86_64",
    "hk-hosted",
];

/// 零依赖 crate：保持纯契约或纯库。
const ZERO_DEPENDENCY: &[&str] = &["hk-shell", "hk-syscall"];

/// 检查所有工作区 crate 的常规依赖方向。
pub fn run() -> Result<()> {
    let root = workspace::root();
    let mut manifests = Vec::new();
    for group in ["crates", "platforms", "servers", "clients", "demos"] {
        let directory = root.join(group);
        let entries = fs::read_dir(&directory)
            .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", directory.display())))?;
        for entry in entries {
            let path = entry
                .map_err(|error| Failure::new(format!("读取目录项失败：{error}")))?
                .path()
                .join("Cargo.toml");
            if path.is_file() {
                manifests.push(path);
            }
        }
    }
    let xtask = root.join("xtask/Cargo.toml");
    if xtask.is_file() {
        manifests.push(xtask);
    }

    let mut violations = Vec::new();
    for path in &manifests {
        check(path, &mut violations)?;
    }
    if !violations.is_empty() {
        return Err(Failure::new(format!(
            "依赖方向违规：\n{}",
            violations.join("\n")
        )));
    }
    println!("依赖方向检查通过：{} 个 crate", manifests.len());
    Ok(())
}

fn check(path: &Path, violations: &mut Vec<String>) -> Result<()> {
    let text = fs::read_to_string(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let manifest: toml::Value = text
        .parse()
        .map_err(|error| Failure::new(format!("解析 {} 失败：{error}", path.display())))?;
    let name = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .unwrap_or_default();
    if name.is_empty() {
        return Ok(());
    }
    let Some(dependencies) = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.as_table())
    else {
        return Ok(());
    };

    if ZERO_DEPENDENCY.contains(&name) {
        for dependency in dependencies.keys() {
            violations.push(format!(
                "  {name} 必须零依赖，但依赖了 {dependency}（{}）",
                path.display()
            ));
        }
        return Ok(());
    }

    let forbidden = rules(name);
    for dependency in dependencies.keys() {
        if forbidden
            .iter()
            .any(|prefix| dependency.starts_with(prefix))
        {
            violations.push(format!(
                "  {name} 不允许依赖 {dependency}（{}）",
                path.display()
            ));
        }
    }
    Ok(())
}

fn rules(name: &str) -> &'static [&'static str] {
    match name {
        "hk-base" | "hk-hal" | "hk-handle" | "hk-ipc" | "hk-mem" | "hk-kernel" | "hk-syscall" => {
            IMPL_FORBIDDEN
        }
        "hk-server-api" => &[
            "hk-handle",
            "hk-kernel",
            "hk-mem",
            "hk-proto",
            "hk-runtime",
            "hk-root-server",
            "hk-platform-",
            "hk-server-",
            "hk-x86_64",
            "hk-hosted",
        ],
        "hk-runtime" => &[
            "hk-root-server",
            "hk-server-",
            "hk-platform-",
            "hk-x86_64",
            "hk-hosted",
        ],
        "hk-root-server" => &["hk-server-", "hk-platform-", "hk-x86_64", "hk-hosted"],
        "hk-fs" => CONTRACT_FORBIDDEN,
        "hk-image" => IMPL_FORBIDDEN,
        "hk-fs-holyfs" => HOLYFS_IMPL_FORBIDDEN,
        "hk-proto" => PROTO_FORBIDDEN,
        "hk-driver-virtio" | "hk-drivers" | "hk-net" | "hk-vfs" | "hk-block" | "hk-fs-ext4"
        | "hk-fs-holyfs-vfs" | "hk-debug" | "hk-canvas" => USERSPACE_LIB_FORBIDDEN,
        "hk-framebuffer" | "hk-acpi" | "hk-boot" | "hk-smp" => LIB_FORBIDDEN,
        service if service.starts_with("hk-server-") => SERVER_FORBIDDEN,
        _ => &[],
    }
}
