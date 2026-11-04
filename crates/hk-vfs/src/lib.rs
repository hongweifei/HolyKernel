//! VFS 核心：路径规范化、挂载表、文件句柄与后端接口。
//!
//! 本 crate 不依赖平台；具体文件系统实现 [`Backend`]，VFS Server 在用户态
//! 持有 `Vfs` 并通过 IPC 暴露操作。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

mod cache;
mod devfs;
mod path;
mod procfs;
mod tmpfs;
mod vfs;

pub use cache::{PAGE_SIZE, PageCache};
pub use devfs::Devfs;
pub use path::normalize;
pub use procfs::Procfs;
pub use tmpfs::Tmpfs;
pub use vfs::{Backend, DirEntry, FileHandle, FileType, Metadata, OpenFlags, Vfs, VfsError};
