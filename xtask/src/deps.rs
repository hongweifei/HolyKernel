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

/// 最底层契约禁止依赖的类别：分页与平台实现必须位于它们之上。
const FOUNDATION_FORBIDDEN: &[&str] = &["hk-fw", "hk-paging", "hk-boot", "hk-platform-"];

/// `hk-fw` 允许的直接依赖：基础契约与固件库。
const FIRMWARE_ALLOWED: &[&str] = &["hk-base", "acpi", "hermit-dtb", "intel-iommu", "amd-iommu"];

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
const ZERO_DEPENDENCY: &[&str] = &["hk-shell", "hk-syscall", "hk-block", "hk-status"];

/// 系统调用薄客户端禁止依赖的类别：只保留 ABI 与零分配基础类型。
const SYSCALL_CLIENT_FORBIDDEN: &[&str] = &[
    "hk-kernel",
    "hk-runtime",
    "hk-root-server",
    "hk-platform-",
    "hk-x86_64",
    "hk-hosted",
    "hk-server-",
    "hk-proto",
    "hk-fs",
    "hk-vfs",
    "hk-block",
    "hk-net",
    "hk-driver",
    "linked_list_allocator",
    "spin",
];

/// crate 侧别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// 只被内核与平台链接。
    Kernel,
    /// 只被用户态服务与客户端链接。
    User,
    /// 内核与用户态共用的契约 crate。
    Shared,
    /// 宿主工具。
    Host,
}

/// 共享契约、内核侧与用户态 crate 的目录。
const SIDE_DIRECTORY: [(&str, Side); 3] = [
    ("kernel", Side::Kernel),
    ("shared", Side::Shared),
    ("user", Side::User),
];

/// 依赖分类来自 crate 所在目录；`servers`、`clients`、`demos` 归用户态，
/// `platforms` 归内核侧。
fn side_of_path(path: &Path, root: &Path) -> Option<Side> {
    for (directory, side) in SIDE_DIRECTORY {
        if path.starts_with(root.join("crates").join(directory)) {
            return Some(side);
        }
    }
    if ["servers", "clients", "demos"]
        .iter()
        .any(|group| path.starts_with(root.join(group)))
    {
        return Some(Side::User);
    }
    if path.starts_with(root.join("platforms")) {
        return Some(Side::Kernel);
    }
    if path == root.join("xtask").join("Cargo.toml") {
        return Some(Side::Host);
    }
    None
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Kernel => "内核",
        Side::User => "用户态",
        Side::Shared => "共享",
        Side::Host => "宿主",
    }
}

fn violates(crate_side: Side, dependency_side: Side) -> bool {
    match crate_side {
        Side::Kernel => dependency_side == Side::User,
        Side::User => dependency_side == Side::Kernel,
        Side::Shared => matches!(dependency_side, Side::Kernel | Side::User),
        Side::Host => false,
    }
}

/// 检查所有工作区 crate 的常规依赖方向。
pub fn run() -> Result<()> {
    let root = workspace::root();
    let mut manifests = Vec::new();
    let mut violations = Vec::new();

    // `crates/` 必须按侧别放在 kernel、user、shared 三个目录下。
    let crates = root.join("crates");
    for entry in fs::read_dir(&crates)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", crates.display())))?
    {
        let directory = entry
            .map_err(|error| Failure::new(format!("读取目录项失败：{error}")))?
            .path();
        if directory.join("Cargo.toml").is_file() {
            violations.push(format!(
                "  crate 必须放在 crates/kernel、crates/user 或 crates/shared 下：{}",
                directory.display()
            ));
            continue;
        }
        if !directory.is_dir() {
            continue;
        }
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

    for group in ["platforms", "servers", "clients", "demos"] {
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

    let mut sides: Vec<(String, Side)> = Vec::new();
    for path in &manifests {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(manifest) = text.parse::<toml::Value>() else {
            continue;
        };
        let Some(name) = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str())
        else {
            continue;
        };
        if let Some(side) = side_of_path(path, &root) {
            sides.push((name.to_string(), side));
        }
    }

    let mut counts = [0_usize; 3];
    for path in &manifests {
        if let Some(side) = check(path, &root, &sides, &mut violations)? {
            match side {
                Side::Kernel => counts[0] += 1,
                Side::User => counts[1] += 1,
                Side::Shared => counts[2] += 1,
                Side::Host => {}
            }
        }
    }
    if !violations.is_empty() {
        return Err(Failure::new(format!(
            "依赖方向违规：\n{}",
            violations.join("\n")
        )));
    }
    println!(
        "依赖方向检查通过：{} 个 crate（内核 {} / 用户态 {} / 共享 {}）",
        manifests.len(),
        counts[0],
        counts[1],
        counts[2]
    );
    Ok(())
}

fn check(
    path: &Path,
    root: &Path,
    sides: &[(String, Side)],
    violations: &mut Vec<String>,
) -> Result<Option<Side>> {
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
        return Err(Failure::new(format!(
            "{} 缺少 package.name",
            path.display()
        )));
    }
    let Some(side) = side_of_path(path, root) else {
        violations.push(format!(
            "  {name} 未登记侧别（{}）；crate 必须放在 crates/kernel、crates/user 或 crates/shared 下",
            path.display()
        ));
        return Ok(None);
    };

    let dependencies = collect_dependencies(&manifest)?;

    if ZERO_DEPENDENCY.contains(&name) {
        for dependency in dependencies
            .iter()
            .filter(|dependency| dependency.kind == Kind::Normal)
        {
            violations.push(format!(
                "  {name} 必须零依赖，但依赖了 {}（{}）",
                dependency.package,
                path.display()
            ));
        }
        return Ok(Some(side));
    }

    for dependency in &dependencies {
        let Some(dependency_side) = sides
            .iter()
            .find(|(name, _)| *name == dependency.package)
            .map(|(_, side)| *side)
        else {
            continue;
        };
        if violates(side, dependency_side) {
            violations.push(format!(
                "  {name}（{}）不允许依赖 {}（{}）—— {}",
                dependency.package,
                side_name(side),
                side_name(dependency_side),
                path.display()
            ));
        }
    }

    let forbidden = rules(name);
    for dependency in dependencies
        .iter()
        .filter(|dependency| dependency.kind == Kind::Normal)
    {
        if forbidden
            .iter()
            .any(|prefix| dependency.package.starts_with(prefix))
        {
            violations.push(format!(
                "  {name} 不允许依赖 {}（{}）",
                dependency.package,
                path.display()
            ));
        }
    }

    if let Some(allowed) = allowed_dependencies(name) {
        for dependency in dependencies
            .iter()
            .filter(|dependency| dependency.kind == Kind::Normal)
        {
            if !allowed.contains(&dependency.package.as_str()) {
                violations.push(format!(
                    "  {name} 只能依赖 {}，但依赖了 {}（{}）",
                    allowed.join("、"),
                    dependency.package,
                    path.display()
                ));
            }
        }
    }
    Ok(Some(side))
}

/// 依赖类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Normal,
    Dev,
    Build,
}

/// 一条依赖：解析后的真实 crate 名与所属依赖段。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Dependency {
    package: String,
    kind: Kind,
}

fn collect_dependencies(manifest: &toml::Value) -> Result<Vec<Dependency>> {
    let Some(table) = manifest.as_table() else {
        return Err(Failure::new("Cargo.toml 顶层不是表"));
    };
    let mut dependencies = Vec::new();
    for (section, kind) in [
        ("dependencies", Kind::Normal),
        ("dev-dependencies", Kind::Dev),
        ("build-dependencies", Kind::Build),
    ] {
        collect_section(table.get(section), kind, &mut dependencies)?;
    }
    if let Some(targets) = table.get("target").and_then(|target| target.as_table()) {
        for target in targets.values() {
            if let Some(target) = target.as_table() {
                for (section, kind) in [
                    ("dependencies", Kind::Normal),
                    ("dev-dependencies", Kind::Dev),
                    ("build-dependencies", Kind::Build),
                ] {
                    collect_section(target.get(section), kind, &mut dependencies)?;
                }
            }
        }
    }
    Ok(dependencies)
}

fn collect_section(
    section: Option<&toml::Value>,
    kind: Kind,
    dependencies: &mut Vec<Dependency>,
) -> Result<()> {
    let Some(section) = section else {
        return Ok(());
    };
    let Some(table) = section.as_table() else {
        return Err(Failure::new("依赖段不是表"));
    };
    for (key, value) in table {
        let package = match value
            .as_table()
            .and_then(|dependency| dependency.get("package"))
        {
            Some(package) => package
                .as_str()
                .ok_or_else(|| Failure::new(format!("依赖 {key} 的 package 字段不是字符串")))?,
            None => key,
        };
        dependencies.push(Dependency {
            package: package.to_string(),
            kind,
        });
    }
    Ok(())
}

fn allowed_dependencies(name: &str) -> Option<&'static [&'static str]> {
    (name == "hk-fw").then_some(FIRMWARE_ALLOWED)
}

fn rules(name: &str) -> &'static [&'static str] {
    match name {
        "hk-base" | "hk-hal" => FOUNDATION_FORBIDDEN,
        "hk-handle" | "hk-ipc" | "hk-mem" | "hk-kernel" | "hk-syscall" => IMPL_FORBIDDEN,
        "hk-syscall-client" => SYSCALL_CLIENT_FORBIDDEN,
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
        "hk-driver-virtio" | "hk-net" | "hk-vfs" | "hk-block" | "hk-block-ipc"
        | "hk-holy-proto" | "hk-proto-client" | "hk-fs-ext4" | "hk-fs-holyfs-vfs" | "hk-debug"
        | "hk-canvas" => USERSPACE_LIB_FORBIDDEN,
        "hk-paging" => USERSPACE_LIB_FORBIDDEN,
        "hk-framebuffer" | "hk-fw" | "hk-boot" | "hk-smp" | "hk-drivers" => LIB_FORBIDDEN,
        service if service.starts_with("hk-server-") => SERVER_FORBIDDEN,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_dependencies_stay_on_the_neutral_layer() {
        assert_eq!(allowed_dependencies("hk-fw"), Some(FIRMWARE_ALLOWED));
        assert_eq!(allowed_dependencies("hk-base"), None);
        assert!(FIRMWARE_ALLOWED.contains(&"hk-base"));
        assert!(!FIRMWARE_ALLOWED.contains(&"hk-paging"));
        assert!(!FIRMWARE_ALLOWED.contains(&"hk-platform-x86_64"));
    }

    #[test]
    fn foundation_crates_cannot_reach_paging_or_platforms() {
        assert!(FOUNDATION_FORBIDDEN.contains(&"hk-paging"));
        assert!(FOUNDATION_FORBIDDEN.contains(&"hk-platform-"));
        assert_eq!(rules("hk-paging"), USERSPACE_LIB_FORBIDDEN);
    }

    #[test]
    fn resolves_renames_and_every_dependency_table() {
        let manifest: toml::Value = r#"
[dependencies]
alias = { package = "hk-fs", path = "../hk-fs" }
plain = "1"

[dev-dependencies]
hk-platform-hosted = { path = "../hk-platform-hosted" }

[build-dependencies]
hk-server-build = { path = "../../crates/user/hk-server-build" }

[target.'cfg(unix)'.dependencies]
renamed = { package = "hk-vfs", path = "../hk-vfs" }
"#
        .parse()
        .expect("清单");
        let dependencies = collect_dependencies(&manifest).expect("收集依赖");
        assert!(dependencies.contains(&Dependency {
            package: "hk-fs".to_string(),
            kind: Kind::Normal,
        }));
        assert!(dependencies.contains(&Dependency {
            package: "hk-platform-hosted".to_string(),
            kind: Kind::Dev,
        }));
        assert!(dependencies.contains(&Dependency {
            package: "hk-server-build".to_string(),
            kind: Kind::Build,
        }));
        assert!(dependencies.contains(&Dependency {
            package: "hk-vfs".to_string(),
            kind: Kind::Normal,
        }));
    }
}
