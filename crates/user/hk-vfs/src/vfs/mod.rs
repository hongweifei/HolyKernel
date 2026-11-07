//! 挂载、路径投影与文件句柄。
//!
//! 后端契约是 `hk_fs::Backend`（DocId/集合/修订）；路径与偏移只作为
//! 兼容投影存在，供旧客户端与 POSIX 层使用。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_fs::{Backend, DocId, Error as FsError, RevisionId};

use crate::cache::PageCache;
use crate::path::normalize;
use crate::path_object::PathObject;

mod handle;
mod mount;
#[cfg(test)]
mod tests;
mod types;

pub use types::{
    DirEntry, FileHandle, FileType, Metadata, NodeId, OpenFlags, PathBackend, VfsError,
};

struct Instance {
    backend: Box<dyn Backend>,
    root: DocId,
}

struct Mount {
    target: String,
    instance: usize,
}

struct OpenFile {
    instance: usize,
    document: DocId,
    revision: RevisionId,
    size: u64,
    offset: u64,
    flags: OpenFlags,
}

const DENTRY_LIMIT: usize = 1024;

/// 虚拟文件系统。
pub struct Vfs {
    instances: Vec<Instance>,
    mounts: Vec<Mount>,
    handles: BTreeMap<u64, OpenFile>,
    dentries: BTreeMap<(usize, DocId, String), DocId>,
    cache: PageCache,
    next_handle: u64,
}

impl Vfs {
    /// 以 Holy 后端构造 VFS。
    pub fn new(root: Box<dyn Backend>) -> Self {
        let root_document = root.root();
        Self {
            instances: alloc::vec![Instance {
                backend: root,
                root: root_document,
            }],
            mounts: Vec::new(),
            handles: BTreeMap::new(),
            dentries: BTreeMap::new(),
            cache: PageCache::new(),
            next_handle: 1,
        }
    }

    /// 以路径后端构造 VFS；后端由 [`PathObject`] 适配。
    pub fn new_path<P: PathBackend + 'static>(root: P) -> Self {
        Self::new(Box::new(PathObject::new(root)))
    }
}

pub(crate) fn map_fs_error(error: FsError) -> VfsError {
    match error {
        FsError::NotFound => VfsError::NotFound,
        FsError::NotDirectory => VfsError::NotDirectory,
        FsError::IsDirectory => VfsError::IsDirectory,
        FsError::Exists => VfsError::AlreadyExists,
        FsError::ReadOnly | FsError::Refused => VfsError::PermissionDenied,
        FsError::Invalid | FsError::Stale => VfsError::InvalidOffset,
        _ => VfsError::Backend,
    }
}
