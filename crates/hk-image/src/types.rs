//! 镜像视图与构建输入类型。

/// Task 记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task<'a> {
    /// 服务名称。
    pub name: &'a str,
    /// 服务角色位。
    pub role: u32,
    /// 调度优先级。
    pub priority: u8,
    /// 入口地址。
    pub entry: u64,
    /// 用户栈大小。
    pub stack_size: u32,
    /// Arena 大小。
    pub arena_size: u32,
}

/// 内存区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region<'a> {
    /// 目标虚拟地址。
    pub virtual_address: u64,
    /// 段在内存中的长度。
    pub memory_size: u64,
    /// 访问标志。
    pub flags: u32,
    /// 文件内容。
    pub file: &'a [u8],
}

/// 能力声明。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability<'a> {
    /// 能力类别。
    pub kind: u8,
    /// 权利位。
    pub rights: u32,
    /// 能力名称。
    pub name: &'a str,
}

/// 调试符号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbol<'a> {
    /// 符号地址。
    pub address: u64,
    /// 符号名称。
    pub name: &'a str,
}

/// 服务依赖。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dependency<'a> {
    /// 依赖服务名。
    pub name: &'a str,
    /// 最低版本。
    pub min_version: u16,
}

/// 构建镜像用的 Task。
#[derive(Clone, Copy, Debug)]
pub struct TaskData<'a> {
    /// 服务名称。
    pub name: &'a str,
    /// 服务角色位。
    pub role: u32,
    /// 调度优先级。
    pub priority: u8,
    /// 入口地址。
    pub entry: u64,
    /// 用户栈大小。
    pub stack_size: u32,
    /// Arena 大小。
    pub arena_size: u32,
}

/// 构建镜像用的内存区域。
#[derive(Clone, Copy, Debug)]
pub struct RegionData<'a> {
    /// 目标虚拟地址。
    pub virtual_address: u64,
    /// 文件内容。
    pub data: &'a [u8],
    /// 段在内存中的长度。
    pub memory_size: u64,
    /// 访问标志。
    pub flags: u32,
}

/// 构建镜像用的能力。
#[derive(Clone, Copy, Debug)]
pub struct CapabilityData<'a> {
    /// 能力类别。
    pub kind: u8,
    /// 权利位。
    pub rights: u32,
    /// 能力名称。
    pub name: &'a str,
}

/// 构建镜像用的符号。
#[derive(Clone, Copy, Debug)]
pub struct SymbolData<'a> {
    /// 符号地址。
    pub address: u64,
    /// 符号名称。
    pub name: &'a str,
}

/// 构建镜像用的依赖。
#[derive(Clone, Copy, Debug)]
pub struct DependencyData<'a> {
    /// 依赖服务名。
    pub name: &'a str,
    /// 最低版本。
    pub min_version: u16,
}
