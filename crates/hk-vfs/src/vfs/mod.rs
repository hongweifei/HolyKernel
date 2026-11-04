//! 挂载表、路径解析、文件句柄与后端接口。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::cache::PageCache;
use crate::path::normalize;

mod handle;
mod mount;
#[cfg(test)]
mod tests;
mod types;

pub use types::{Backend, DirEntry, FileHandle, FileType, Metadata, NodeId, OpenFlags, VfsError};

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

const DENTRY_LIMIT: usize = 1024;

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
}
