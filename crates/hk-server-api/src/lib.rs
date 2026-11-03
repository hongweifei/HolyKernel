//! 服务器与内核之间的入口 ABI。
//!
//! 内核把 [`ServerApi`] 的引用交给服务入口；服务只通过这张函数表与内核交互，
//! 不直接链接内核符号。

#![no_std]

/// 内核提供给服务器的函数表。
#[repr(C)]
pub struct ServerApi {
    /// 把 UTF-8 文本写入内核控制台。
    pub write: extern "C" fn(*const u8, usize),
}

/// 服务器入口签名。
pub type ServerEntry = extern "C" fn(&ServerApi);
