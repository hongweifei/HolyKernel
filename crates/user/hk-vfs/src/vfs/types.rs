//! VFS 公共类型与后端契约。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use hk_fs::Status;

/// 后端节点编号。
pub type NodeId = u64;

/// 文件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// 普通文件。
    File,
    /// 目录。
    Directory,
}

/// 文件元数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    /// 文件类型。
    pub kind: FileType,
    /// 文件大小。
    pub size: u64,
}

/// 目录项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// 名称。
    pub name: String,
    /// 类型。
    pub kind: FileType,
}

/// 打开标志。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenFlags {
    /// 允许读取。
    pub read: bool,
    /// 允许写入。
    pub write: bool,
    /// 不存在时创建。
    pub create: bool,
}

impl OpenFlags {
    /// 只读打开。
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        create: false,
    };

    /// 只写打开；不存在时失败。
    pub const WRITE_ONLY: Self = Self {
        read: false,
        write: true,
        create: false,
    };

    /// 读写打开。
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        create: false,
    };

    /// 读写打开，不存在时创建。
    pub const CREATE: Self = Self {
        read: true,
        write: true,
        create: true,
    };
}

/// 文件句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileHandle(pub(crate) u64);

impl FileHandle {
    /// 从协议句柄编号恢复。
    pub const fn from_id(id: u64) -> Self {
        Self(id)
    }

    /// 返回句柄编号。
    pub const fn id(self) -> u64 {
        self.0
    }
}

/// VFS 错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfsError {
    /// 路径或节点不存在。
    NotFound,
    /// 父节点不是目录。
    NotDirectory,
    /// 对目录执行了文件操作。
    IsDirectory,
    /// 路径非法。
    InvalidPath,
    /// 节点已存在。
    AlreadyExists,
    /// 打开标志不允许该操作。
    PermissionDenied,
    /// 只读文件系统。
    ReadOnly,
    /// 偏移溢出。
    InvalidOffset,
    /// 后端错误。
    Backend,
}

impl VfsError {
    /// 映射到服务状态；新增错误变体时只需在这里补一处。
    pub const fn status(self) -> Status {
        match self {
            Self::NotFound => Status::NotFound,
            Self::InvalidPath => Status::Invalid,
            Self::PermissionDenied => Status::Permission,
            Self::AlreadyExists => Status::Exists,
            Self::NotDirectory
            | Self::IsDirectory
            | Self::ReadOnly
            | Self::InvalidOffset
            | Self::Backend => Status::Error,
        }
    }
}

impl fmt::Display for VfsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "路径不存在",
            Self::NotDirectory => "不是目录",
            Self::IsDirectory => "是目录",
            Self::InvalidPath => "路径非法",
            Self::AlreadyExists => "已存在",
            Self::PermissionDenied => "权限不足",
            Self::ReadOnly => "只读文件系统",
            Self::InvalidOffset => "偏移非法",
            Self::Backend => "文件系统后端错误",
        })
    }
}

impl core::error::Error for VfsError {}

/// 路径形状的文件系统后端。
///
/// 外来文件系统（ext4、tmpfs、devfs、procfs）实现本契约，再由
/// [`crate::PathObject`] 适配为 Holy 后端。
pub trait PathBackend {
    /// 返回根节点。
    fn root(&self) -> NodeId;
    /// 在目录中查找名称。
    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError>;
    /// 读取元数据。
    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError>;
    /// 从文件读取。
    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError>;
    /// 向文件写入。
    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError>;
    /// 在目录中创建节点。
    fn create(&mut self, parent: NodeId, name: &str, kind: FileType) -> Result<NodeId, VfsError>;
    /// 读取目录项。
    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError>;
    /// 调整文件大小。
    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError>;
    /// 删除目录项，返回被删除的节点。
    fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, VfsError>;
    /// 该节点是否使用 VFS 页缓存；设备节点应返回 `false`。
    fn cacheable(&self, _node: NodeId) -> bool {
        true
    }

    /// 把后端缓存与挂起写入刷到稳定存储。
    fn sync(&mut self) -> Result<(), VfsError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_errors_to_status() {
        assert_eq!(VfsError::NotFound.status(), Status::NotFound);
        assert_eq!(VfsError::InvalidPath.status(), Status::Invalid);
        assert_eq!(VfsError::PermissionDenied.status(), Status::Permission);
        assert_eq!(VfsError::AlreadyExists.status(), Status::Exists);
        assert_eq!(VfsError::Backend.status(), Status::Error);
    }
}
