//! Holy 文档服务的共享运行时。
//!
//! 服务只负责提供后端与盘符元数据；本 crate 统一维护盘符注册表并把 Holy
//! 请求分派到对应后端。VFS 与独立的 ext4 服务因此共用同一套协议处理。

#![no_std]

extern crate alloc;

mod drives;
mod handle;

pub use drives::{Drive, Drives, LOCAL_VOLUME_BASE};
pub use handle::handle;
