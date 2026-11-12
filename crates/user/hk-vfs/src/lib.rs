//! VFS 核心：Holy 后端契约、路径投影、挂载表与文件句柄。
//!
//! 命名空间以 `DocId`、集合与修订为核心；路径后端通过 [`PathObject`]
//! 适配，路径与偏移是 POSIX/Linux/Harmony 兼容投影。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

mod block;
mod cache;
mod devfs;
mod path;
mod path_object;
mod procfs;
mod tmpfs;
mod vfs;

pub use block::BlockBackend;
pub use cache::{PAGE_SIZE, PageCache};
pub use devfs::Devfs;
pub use hk_fs::{DocId, RevisionId};
pub use path::normalize;
pub use path_object::PathObject;
pub use procfs::Procfs;
pub use tmpfs::Tmpfs;
pub use vfs::{
    DirEntry, FileHandle, FileType, Metadata, NodeId, OpenFlags, PathBackend, Vfs, VfsError,
};
