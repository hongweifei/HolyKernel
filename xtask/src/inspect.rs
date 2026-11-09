//! 原生镜像查看。

use std::path::PathBuf;

use clap::Args;
use hk_image::{CAP_DEVICE, CAP_ENDPOINT, CAP_NOTIFICATION, FILE_SUFFIX, Image};

use crate::catalog::SERVERS;
use crate::failure::{Failure, Result};
use crate::native::{read_image, service_image};
use crate::workspace;

#[derive(Debug, Args)]
pub struct InspectArguments {
    /// 服务名；省略时汇总 build/iso 中的全部服务
    #[arg(long, conflicts_with = "path")]
    pub service: Option<String>,
    /// 直接检查镜像文件
    #[arg(long, conflicts_with = "service")]
    pub path: Option<PathBuf>,
    /// 使用 release 构建产物
    #[arg(long)]
    pub release: bool,
    /// 列出符号
    #[arg(long)]
    pub symbols: bool,
    /// 符号显示上限
    #[arg(long, default_value_t = 40)]
    pub limit: usize,
}

pub fn run(arguments: InspectArguments) -> Result<()> {
    if let Some(path) = arguments.path {
        let bytes = read_image(&path)?;
        show(
            &path.display().to_string(),
            &bytes,
            arguments.symbols,
            arguments.limit,
        )
    } else if let Some(service) = arguments.service {
        let (path, bytes) = service_image(&service, arguments.release)?;
        show(&path, &bytes, arguments.symbols, arguments.limit)
    } else {
        summary()
    }
}

fn show(path: &str, bytes: &[u8], symbols: bool, limit: usize) -> Result<()> {
    let image = Image::parse(bytes)
        .ok_or_else(|| Failure::new(format!("{path} 不是有效的原生任务镜像")))?;
    let task = image
        .task()
        .ok_or_else(|| Failure::new(format!("{path} 缺少 Task 记录")))?;
    println!("镜像：{path}");
    println!(
        "  标识：{:#018x}  大小：{} 字节",
        image.image_id(),
        bytes.len()
    );
    println!("Task：{}", task.name);
    println!("  角色：{:#x}  入口：{:#018x}", task.role, task.entry);
    println!(
        "  栈：{} KiB  Arena：{} KiB",
        task.stack_size / 1024,
        task.arena_size / 1024
    );

    let regions: Vec<_> = image.regions().collect();
    println!("Region：{} 项", regions.len());
    for region in &regions {
        println!(
            "  {:#018x} file={} mem={} flags={:#x}",
            region.virtual_address,
            region.file.len(),
            region.memory_size,
            region.flags
        );
    }

    let capabilities: Vec<_> = image.capabilities().collect();
    if capabilities.is_empty() {
        println!("Capability：无");
    } else {
        println!("Capability：{} 项", capabilities.len());
        for capability in &capabilities {
            println!(
                "  {} rights={:#x} name={}",
                capability_name(capability.kind),
                capability.rights,
                capability.name
            );
        }
    }

    let dependencies: Vec<_> = image.dependencies().collect();
    if dependencies.is_empty() {
        println!("Dependency：无");
    } else {
        println!("Dependency：{} 项", dependencies.len());
        for dependency in &dependencies {
            println!("  {} >= {}", dependency.name, dependency.min_version);
        }
    }

    let all_symbols: Vec<_> = image.symbols().collect();
    println!("Symbol：{} 项", all_symbols.len());
    if symbols {
        for symbol in all_symbols.iter().take(limit) {
            println!("  {:#018x} {}", symbol.address, symbol.name);
        }
        if all_symbols.len() > limit {
            println!("  … 其余 {} 项", all_symbols.len() - limit);
        }
    }
    Ok(())
}

fn summary() -> Result<()> {
    let directory = workspace::root().join("build/iso/boot");
    if !directory.is_dir() {
        return Err(Failure::new(
            "未找到 build/iso/boot；先运行 cargo xtask image",
        ));
    }
    println!(
        "{:<28} {:>7} {:>8} {:>7} 能力",
        "服务", "Region", "Symbol", "优先级"
    );
    for service in SERVERS {
        let path = directory.join(format!("{service}{FILE_SUFFIX}"));
        if !path.is_file() {
            continue;
        }
        let Ok(bytes) = read_image(&path) else {
            continue;
        };
        let Some(image) = Image::parse(&bytes) else {
            println!("{service:<28} 镜像无效");
            continue;
        };
        if image.task().is_none() {
            continue;
        }
        let capabilities: Vec<_> = image
            .capabilities()
            .map(|capability| capability_name(capability.kind))
            .collect();
        let capabilities = if capabilities.is_empty() {
            "-".to_string()
        } else {
            capabilities.join(",")
        };
        println!(
            "{service:<28} {:>7} {:>8} {:>7} {}",
            image.regions().count(),
            image.symbols().count(),
            image.capabilities().count(),
            capabilities
        );
    }
    Ok(())
}

fn capability_name(kind: u8) -> &'static str {
    match kind {
        CAP_DEVICE => "device",
        CAP_ENDPOINT => "endpoint",
        CAP_NOTIFICATION => "notification",
        _ => "other",
    }
}
