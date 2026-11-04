//! HolyShell 命令解析与输出。
//!
//! 本 crate 只把命令行转换成命令与响应文本，不接触硬件；平台负责输入、输出与
//! 执行副作用（清屏、退出、装载服务）。

#![no_std]

extern crate alloc;

mod command;
mod execute;
mod format;
mod host;
mod response;
mod session;

pub use command::{Command, parse};
pub use execute::{execute, execute_line, run_repl, run_script};
pub use host::{ShellHost, ShellIo, ShellState};
pub use response::Response;
pub use session::Session;

#[cfg(test)]
mod tests;
