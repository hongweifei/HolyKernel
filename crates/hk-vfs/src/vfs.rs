//! 挂载表、路径解析、文件句柄与后端接口。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::cache::PageCache;
use crate::path::normalize;

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
pub struct FileHandle(u64);

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
    /// 偏移溢出。
    InvalidOffset,
    /// 后端错误。
    Backend,
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
            Self::InvalidOffset => "偏移非法",
            Self::Backend => "文件系统后端错误",
        })
    }
}

impl core::error::Error for VfsError {}

/// 文件系统后端。
pub trait Backend {
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
}

struct Instance {
    backend: Box<dyn Backend>,
    root: NodeId,
}

struct Mount {
    target: String,
    instance: usize,
}

struct OpenFile {
    instance: usize,
    node: NodeId,
    offset: u64,
    flags: OpenFlags,
}

/// 虚拟文件系统。
pub struct Vfs {
    instances: Vec<Instance>,
    mounts: Vec<Mount>,
    handles: BTreeMap<u64, OpenFile>,
    dentries: BTreeMap<(usize, NodeId, String), NodeId>,
    cache: PageCache,
    next_handle: u64,
}

impl Vfs {
    /// 以根文件系统构造 VFS。
    pub fn new(root: Box<dyn Backend>) -> Self {
        let root_node = root.root();
        Self {
            instances: alloc::vec![Instance {
                backend: root,
                root: root_node,
            }],
            mounts: Vec::new(),
            handles: BTreeMap::new(),
            dentries: BTreeMap::new(),
            cache: PageCache::new(),
            next_handle: 1,
        }
    }

    /// 在挂载点安装后端。
    pub fn mount(&mut self, target: &str, backend: Box<dyn Backend>) -> Result<(), VfsError> {
        let target = normalize(target);
        if target == "/" {
            self.dentries.clear();
            self.cache.clear();
            let root = backend.root();
            self.instances[0] = Instance { backend, root };
            return Ok(());
        }
        if self.mounts.iter().any(|mount| mount.target == target) {
            return Err(VfsError::AlreadyExists);
        }

        let instance = self.instances.len();
        let root = backend.root();
        self.dentries.clear();
        self.cache.clear();
        self.instances.push(Instance { backend, root });
        self.mounts.push(Mount { target, instance });
        Ok(())
    }

    /// 打开文件，返回句柄。
    pub fn open(&mut self, path: &str, flags: OpenFlags) -> Result<FileHandle, VfsError> {
        let path = normalize(path);
        let (instance, node) = match self.resolve(&path) {
            Ok((instance, node)) => {
                let metadata = self.instances[instance]
                    .backend
                    .metadata(node)
                    .map_err(|_| VfsError::Backend)?;
                if metadata.kind == FileType::Directory && flags.write {
                    return Err(VfsError::IsDirectory);
                }
                (instance, node)
            }
            Err(VfsError::NotFound) if flags.create => self.create_node(&path, FileType::File)?,
            Err(error) => return Err(error),
        };

        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(VfsError::Backend)?;
        self.handles.insert(
            handle,
            OpenFile {
                instance,
                node,
                offset: 0,
                flags,
            },
        );
        Ok(FileHandle(handle))
    }

    /// 关闭文件句柄。
    pub fn close(&mut self, handle: FileHandle) -> Result<(), VfsError> {
        self.handles
            .remove(&handle.0)
            .map(|_| ())
            .ok_or(VfsError::NotFound)
    }

    /// 从文件句柄的指定偏移读取。
    pub fn read_at(
        &mut self,
        handle: FileHandle,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        let (instance, node) = {
            let open = self.handles.get(&handle.0).ok_or(VfsError::NotFound)?;
            if !open.flags.read {
                return Err(VfsError::PermissionDenied);
            }
            (open.instance, open.node)
        };
        self.cache.read(
            &*self.instances[instance].backend,
            instance,
            node,
            offset,
            buffer,
        )
    }

    /// 从文件句柄读取，并推进偏移。
    pub fn read(&mut self, handle: FileHandle, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let offset = self
            .handles
            .get(&handle.0)
            .ok_or(VfsError::NotFound)?
            .offset;
        let read = self.read_at(handle, offset, buffer)?;
        if let Some(open) = self.handles.get_mut(&handle.0) {
            open.offset = open
                .offset
                .checked_add(read as u64)
                .ok_or(VfsError::InvalidOffset)?;
        }
        Ok(read)
    }

    /// 向文件句柄的指定偏移写入。
    pub fn write_at(
        &mut self,
        handle: FileHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        let (instance, node) = {
            let open = self.handles.get(&handle.0).ok_or(VfsError::NotFound)?;
            if !open.flags.write {
                return Err(VfsError::PermissionDenied);
            }
            (open.instance, open.node)
        };
        self.cache.write(
            &mut *self.instances[instance].backend,
            instance,
            node,
            offset,
            data,
        )
    }

    /// 向文件句柄写入，并推进偏移。
    pub fn write(&mut self, handle: FileHandle, data: &[u8]) -> Result<usize, VfsError> {
        let offset = self
            .handles
            .get(&handle.0)
            .ok_or(VfsError::NotFound)?
            .offset;
        let written = self.write_at(handle, offset, data)?;
        if let Some(open) = self.handles.get_mut(&handle.0) {
            open.offset = open
                .offset
                .checked_add(written as u64)
                .ok_or(VfsError::InvalidOffset)?;
        }
        Ok(written)
    }

    /// 读取元数据。
    pub fn metadata(&mut self, path: &str) -> Result<Metadata, VfsError> {
        let (instance, node) = self.resolve(path)?;
        self.instances[instance].backend.metadata(node)
    }

    /// 读取目录项。
    pub fn readdir(&mut self, path: &str) -> Result<Vec<DirEntry>, VfsError> {
        let (instance, node) = self.resolve(path)?;
        self.instances[instance].backend.readdir(node)
    }

    /// 创建目录。
    pub fn create_dir(&mut self, path: &str) -> Result<(), VfsError> {
        self.create_node(path, FileType::Directory).map(|_| ())
    }

    /// 调整文件大小。
    pub fn truncate(&mut self, path: &str, size: u64) -> Result<(), VfsError> {
        let (instance, node) = self.resolve(path)?;
        self.instances[instance].backend.truncate(node, size)?;
        self.cache.invalidate(instance, node);
        Ok(())
    }

    /// 删除文件或空目录。
    pub fn unlink(&mut self, path: &str) -> Result<(), VfsError> {
        let (parent, name) = split_parent(path)?;
        let (instance, parent_node) = self.resolve(&parent)?;
        let node = self.instances[instance]
            .backend
            .unlink(parent_node, &name)?;
        self.dentries.remove(&(instance, parent_node, name));
        self.cache.invalidate(instance, node);
        Ok(())
    }

    /// 返回挂载点。
    pub fn mounts(&self) -> Vec<String> {
        let mut mounts = alloc::vec![String::from("/")];
        for mount in &self.mounts {
            mounts.push(mount.target.clone());
        }
        mounts
    }

    fn create_node(&mut self, path: &str, kind: FileType) -> Result<(usize, NodeId), VfsError> {
        let (parent, name) = split_parent(path)?;
        let (instance, parent_node) = self.resolve(&parent)?;
        let metadata = self.instances[instance].backend.metadata(parent_node)?;
        if metadata.kind != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        let node = self.instances[instance]
            .backend
            .create(parent_node, &name, kind)?;
        self.dentries.insert((instance, parent_node, name), node);
        Ok((instance, node))
    }

    fn resolve(&mut self, path: &str) -> Result<(usize, NodeId), VfsError> {
        let path = normalize(path);
        let mut instance = 0;
        let mut node = self.instances[0].root;
        if let Some((mounted, root)) = self.mount_exact("/") {
            instance = mounted;
            node = root;
        }

        let mut prefix = String::new();
        for component in path.split('/').filter(|component| !component.is_empty()) {
            prefix.push('/');
            prefix.push_str(component);
            if let Some((mounted, root)) = self.mount_exact(&prefix) {
                instance = mounted;
                node = root;
                continue;
            }
            let key = (instance, node, component.to_string());
            if let Some(cached) = self.dentries.get(&key).copied() {
                node = cached;
                continue;
            }
            let found = self.instances[instance].backend.lookup(node, component)?;
            self.dentries.insert(key, found);
            node = found;
        }
        Ok((instance, node))
    }

    fn mount_exact(&self, target: &str) -> Option<(usize, NodeId)> {
        if target == "/" {
            return Some((0, self.instances[0].root));
        }
        self.mounts
            .iter()
            .find(|mount| mount.target == target)
            .map(|mount| {
                let instance = &self.instances[mount.instance];
                (mount.instance, instance.root)
            })
    }
}

fn split_parent(path: &str) -> Result<(String, String), VfsError> {
    let path = normalize(path);
    let (parent, name) = path.rsplit_once('/').ok_or(VfsError::InvalidPath)?;
    if name.is_empty() {
        return Err(VfsError::InvalidPath);
    }
    let parent = if parent.is_empty() {
        String::from("/")
    } else {
        parent.to_string()
    };
    Ok((parent, name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tmpfs;

    #[test]
    fn creates_writes_and_reads_files() {
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
        vfs.create_dir("/tmp").expect("创建目录");
        let handle = vfs.open("/tmp/hello", OpenFlags::CREATE).expect("打开文件");
        assert_eq!(vfs.write(handle, b"hello").expect("写入"), 5);
        vfs.close(handle).expect("关闭");

        let handle = vfs
            .open("/tmp/hello", OpenFlags::READ_ONLY)
            .expect("重新打开");
        let mut buffer = [0_u8; 16];
        let read = vfs.read(handle, &mut buffer).expect("读取");
        assert_eq!(&buffer[..read], b"hello");
        assert_eq!(vfs.metadata("/tmp/hello").expect("元数据").size, 5);
        assert_eq!(vfs.readdir("/tmp").expect("目录").len(), 1);
    }

    #[test]
    fn mounts_cross_path_boundaries() {
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
        vfs.create_dir("/mnt").expect("创建挂载点");
        vfs.mount("/mnt", Box::new(Tmpfs::new())).expect("挂载");

        let handle = vfs.open("/mnt/file", OpenFlags::CREATE).expect("打开");
        vfs.write(handle, b"mounted").expect("写入");
        vfs.close(handle).expect("关闭");

        let handle = vfs.open("/mnt/file", OpenFlags::READ_ONLY).expect("打开");
        let mut buffer = [0_u8; 8];
        let read = vfs.read(handle, &mut buffer).expect("读取");
        assert_eq!(&buffer[..read], b"mounted");
        assert!(matches!(
            vfs.open("/mnt", OpenFlags::WRITE_ONLY),
            Err(VfsError::IsDirectory)
        ));
    }

    #[test]
    fn rejects_invalid_parents() {
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
        assert!(matches!(
            vfs.open("/missing/file", OpenFlags::CREATE),
            Err(VfsError::NotFound)
        ));
        let handle = vfs.open("/file", OpenFlags::CREATE).expect("创建文件");
        vfs.close(handle).expect("关闭");
        assert!(matches!(
            vfs.open("/file/child", OpenFlags::CREATE),
            Err(VfsError::NotDirectory)
        ));
    }
    #[test]
    fn truncates_unlinks_and_reads_at_offsets() {
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
        let handle = vfs.open("/file", OpenFlags::CREATE).expect("创建");
        vfs.write(handle, b"hello").expect("写入");
        let mut buffer = [0_u8; 8];
        assert_eq!(vfs.read_at(handle, 1, &mut buffer).expect("偏移读"), 4);
        assert_eq!(&buffer[..4], b"ello");
        vfs.truncate("/file", 2).expect("截断");
        assert_eq!(vfs.metadata("/file").expect("元数据").size, 2);
        vfs.close(handle).expect("关闭");
        vfs.unlink("/file").expect("删除");
        assert!(matches!(vfs.metadata("/file"), Err(VfsError::NotFound)));
    }

    #[test]
    fn sequential_read_1mib() {
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
        let handle = vfs.open("/large", OpenFlags::CREATE).expect("创建");
        let chunk = [0x5A_u8; 4096];
        for _ in 0..256 {
            vfs.write(handle, &chunk).expect("写入");
        }
        vfs.close(handle).expect("关闭");

        let handle = vfs.open("/large", OpenFlags::READ_ONLY).expect("打开");
        let start = std::time::Instant::now();
        let mut buffer = [0_u8; 4096];
        let mut total = 0;
        loop {
            let read = vfs.read(handle, &mut buffer).expect("读取");
            if read == 0 {
                break;
            }
            total += read;
        }
        let elapsed = start.elapsed();
        assert_eq!(total, 1024 * 1024);
        std::eprintln!(
            "VFS 顺序读：{} MiB/s",
            (total as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64()
        );
    }
}
