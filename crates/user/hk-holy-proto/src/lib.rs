//! HolyKernel 原生文档协议的共享响应构造。
//!
//! 线协议与状态码在 `hk-proto`，文档与令牌会话在 `hk-fs`；本 crate 把两者
//! 接成两个 Holy 文档服务共用的响应层，并保持可单独测试。

#![no_std]

pub mod response;
