//! 用户镜像装载：ELF 段、用户栈与 Arena。
//!
//! 页的分配与映射由平台的 [`UserMemory`] 提供；本模块只负责布局与镜像解析。

use core::fmt;

use hk_elf::ElfError;
use hk_hal::{PAGE_SIZE, UserMemory};

/// 镜像装载错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// ELF 结构或段映射失败。
    Elf(ElfError),
    /// 物理帧耗尽或页映射失败。
    OutOfMemory,
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Elf(ElfError::Parse) => formatter.write_str("ELF 结构无效"),
            Self::Elf(ElfError::Map) => formatter.write_str("ELF 段映射失败"),
            Self::OutOfMemory => formatter.write_str("物理帧耗尽或页映射失败"),
        }
    }
}

/// 镜像布局。
#[derive(Clone, Copy, Debug)]
pub struct ImageSpec<'a> {
    /// ELF 文件内容。
    pub bytes: &'a [u8],
    /// 入口首个参数：内核分配的端点编号。
    pub endpoint: u64,
    /// 用户栈基址。
    pub stack_base: u64,
    /// 用户栈大小。
    pub stack_size: u64,
    /// Arena 基址。
    pub arena_base: u64,
    /// Arena 大小。
    pub arena_size: u64,
}

/// 装载镜像、映射栈与 Arena，返回入口现场。
pub fn load<C, M, F>(spec: &ImageSpec<'_>, memory: &mut M, make_context: F) -> Result<C, LoadError>
where
    M: UserMemory,
    F: FnOnce(u64, u64, u64) -> C,
{
    let entry = hk_elf::load_into(spec.bytes, memory).map_err(LoadError::Elf)?;
    map_region(memory, spec.stack_base, spec.stack_size)?;
    map_region(memory, spec.arena_base, spec.arena_size)?;
    Ok(make_context(
        entry,
        spec.stack_base + spec.stack_size,
        spec.endpoint,
    ))
}

fn map_region<M: UserMemory>(memory: &mut M, base: u64, size: u64) -> Result<(), LoadError> {
    let page_size = PAGE_SIZE as u64;
    let pages = size.div_ceil(page_size);
    for page in 0..pages {
        memory
            .map_page(base + page * page_size)
            .ok_or(LoadError::OutOfMemory)?;
    }
    Ok(())
}

/// 取模块路径的最后一段作为服务名。
pub fn service_name(name: &'static str) -> &'static str {
    name.rsplit('/').next().unwrap_or(name)
}
