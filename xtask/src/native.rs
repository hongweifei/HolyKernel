//! 构建期 ELF → 原生任务镜像转换与符号提取。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use hk_image::{CAP_DEVICE, CapabilityData, FILE_SUFFIX, RegionData, SymbolData, TaskData, build};
use xmas_elf::ElfFile;
use xmas_elf::program::Type as ProgramType;

use crate::failure::{Failure, Result};
use crate::image::binary_path;
use crate::workspace;

/// 读取服务原生镜像；缓存比目标 ELF 新时复用，否则现场转换。
pub(crate) fn service_image(service: &str, release: bool) -> Result<(String, Vec<u8>)> {
    let iso = workspace::root()
        .join("build/iso/boot")
        .join(format!("{service}{FILE_SUFFIX}"));
    let binary = binary_path(service, release);
    if fresh(&iso, &binary) {
        return Ok((iso.display().to_string(), read_image(&iso)?));
    }
    if !binary.is_file() {
        return Err(Failure::new(format!(
            "未找到 {service}；先运行 cargo xtask image 或 cargo xtask build --target x86_64-qemu"
        )));
    }
    Ok((
        format!("{}（构建期转换）", binary.display()),
        native_image(&binary)?,
    ))
}

/// 读取构建期内核符号表；缺失时重新生成。
pub(crate) fn kernel_symbols(release: bool) -> Result<Vec<(u64, String)>> {
    let path = workspace::root().join("build/kernel.sym");
    let binary = binary_path("hk-x86_64", release);
    let path = if fresh(&path, &binary) {
        path
    } else {
        generate_symbols(release)?
    };
    let text = fs::read_to_string(&path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let mut symbols = Vec::new();
    for line in text.lines() {
        let Some((address, name)) = line.split_once(' ') else {
            continue;
        };
        let Ok(address) = u64::from_str_radix(address, 16) else {
            continue;
        };
        symbols.push((address, name.to_string()));
    }
    Ok(symbols)
}

pub(crate) fn read_image(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))
}

/// 缓存文件是否比源 ELF 新。
fn fresh(cache: &Path, source: &Path) -> bool {
    let Ok(cache) = fs::metadata(cache).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let Ok(source) = fs::metadata(source).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    cache >= source
}

/// 镜像内的符号表模块名。
pub(crate) const SYMBOL_MODULE: &str = "kernel.sym";

/// 默认用户栈大小。
const USER_STACK_SIZE: u32 = 512 * 1024;
/// 默认 Arena 大小。
const USER_ARENA_SIZE: u32 = 2 * 1024 * 1024;

/// 生成内核符号表；服务符号随原生镜像携带。
pub(crate) fn generate_symbols(release: bool) -> Result<PathBuf> {
    let mut symbols = collect_symbols(&binary_path("hk-x86_64", release), "[kernel] ")?;
    symbols.sort_by_key(|(address, _)| *address);
    let mut text = String::new();
    for (address, name) in symbols {
        text.push_str(&format!("{address:016x} {name}\n"));
    }
    let path = workspace::root().join("build").join(SYMBOL_MODULE);
    fs::create_dir_all(path.parent().expect("symbol path has parent"))
        .map_err(|error| Failure::new(format!("创建 build 目录失败：{error}")))?;
    fs::write(&path, text)
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(path)
}

/// 把服务 ELF 转换为原生任务镜像。
pub(crate) fn native_image(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let file = ElfFile::new(&bytes)
        .map_err(|error| Failure::new(format!("解析 {} 的 ELF 失败：{error}", path.display())))?;
    let entry = file.header.pt2.entry_point();
    let role = service_role(&file);
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::new("服务镜像文件名无效"))?;
    let mut regions = Vec::new();
    for program in file.program_iter() {
        if program.get_type() != Ok(ProgramType::Load) {
            continue;
        }
        let offset = usize::try_from(program.offset())
            .map_err(|_| Failure::new(format!("{} 段偏移过大", path.display())))?;
        let size = usize::try_from(program.file_size())
            .map_err(|_| Failure::new(format!("{} 段过大", path.display())))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| Failure::new(format!("{} 段越界", path.display())))?;
        let data = bytes
            .get(offset..end)
            .ok_or_else(|| Failure::new(format!("{} 段越界", path.display())))?;
        regions.push(RegionData {
            virtual_address: program.virtual_addr(),
            data,
            memory_size: program.mem_size(),
            flags: program.flags().0,
        });
    }
    let symbols = collect_symbols(path, &format!("[{name}] "))?;
    let symbol_data: Vec<SymbolData> = symbols
        .iter()
        .map(|(address, name)| SymbolData {
            address: *address,
            name,
        })
        .collect();
    let capabilities = if role & hk_image::ServiceRole::DEVICE != 0 {
        vec![CapabilityData {
            kind: CAP_DEVICE,
            rights: 3,
            name: "device",
        }]
    } else {
        Vec::new()
    };
    Ok(build(
        TaskData {
            name,
            role,
            entry,
            stack_size: USER_STACK_SIZE,
            arena_size: USER_ARENA_SIZE,
        },
        &regions,
        &capabilities,
        &symbol_data,
        &[],
    ))
}

fn collect_symbols(binary: &Path, prefix: &str) -> Result<Vec<(u64, String)>> {
    let output = Command::new("nm")
        .args(["-n", "-C", "--defined-only"])
        .arg(binary)
        .output()
        .map_err(|error| Failure::new(format!("执行 nm 失败：{error}")))?;
    if !output.status.success() {
        return Err(Failure::new(format!(
            "nm 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut symbols = Vec::new();
    for line in text.lines() {
        let Some((address, kind, name)) = parse_symbol_line(line) else {
            continue;
        };
        if !matches!(kind, "t" | "T" | "w" | "W") {
            continue;
        }
        symbols.push((address, format!("{prefix}{name}")));
    }
    Ok(symbols)
}

fn service_role(file: &ElfFile<'_>) -> u32 {
    for section in file.section_iter() {
        if section.get_name(file).ok() != Some(hk_image::SERVICE_ROLE_SECTION) {
            continue;
        }
        return hk_image::ServiceRole::parse(section.raw_data(file));
    }
    0
}

fn parse_symbol_line(line: &str) -> Option<(u64, &str, &str)> {
    let line = line.trim();
    let address_end = line.find(char::is_whitespace)?;
    let address = u64::from_str_radix(&line[..address_end], 16).ok()?;
    let rest = line[address_end..].trim_start();
    let kind_end = rest.find(char::is_whitespace)?;
    let kind = &rest[..kind_end];
    let name = rest[kind_end..].trim();
    (!name.is_empty()).then_some((address, kind, name))
}
