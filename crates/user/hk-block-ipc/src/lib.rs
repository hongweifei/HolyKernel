//! 块服务线协议运行时：远端客户端、服务端循环与直写帧登记。
//!
//! 协议编码在 [`hk_proto::block`]；本 crate 把协议接到 [`hk_block`] 的
//! `AsyncBlockDevice` 与 `BlockDevice` 契约，供 Block Server、virtio 驱动服务
//! 与 VFS 复用。
//!
//! 客户端使用多个共享帧槽位与递增标签，服务端按发送者与槽位分别映射帧，
//! 两边都只维护一个收发缓冲，数据面由帧承载。服务循环收下请求后立即提交给
//! 设备，完成回收时再回复，因此同一时刻可以有多条请求在设备队列中。

#![no_std]

extern crate alloc;

pub mod adapter;
pub mod client;
pub mod server;
mod status;

pub use adapter::{register_client_mapping, unregister_client_mapping};
pub use client::{BlockClient, ClientDisk};
pub use server::{serve, serve_with_hook, serve_with_wait, serve_with_wait_and};
