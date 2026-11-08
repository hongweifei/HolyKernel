//! 协议客户端：按需分配收发缓冲。
//!
//! 线协议与编解码在 `hk-proto`，本 crate 只承载需要堆缓冲的客户端。
//! 缓冲在首次使用时分配，客户端结构只保留小的连接状态与服务句柄。

#![no_std]

extern crate alloc;

pub mod holy;
pub mod vfs;

pub use holy::HolyClient;
pub use vfs::VfsClient;
