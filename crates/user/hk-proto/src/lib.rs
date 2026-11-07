//! 服务线协议与固定缓冲客户端。
//!
//! 本 crate 按服务组织线协议：每个模块同时提供给服务端与客户端使用，只依赖
//! 内核面（`hk-server-api`）与 ABI 常量（`hk-syscall`）。共享响应形状与状态码
//! 在 [`status`]，协议标签区间在 [`labels`]。

#![no_std]

pub mod block;
pub mod checkpoint;
pub mod console;
pub mod display;
pub mod echo;
pub mod holy;
pub mod input;
pub mod labels;
pub mod net;
pub mod socket;
pub mod status;
pub mod vfs;
