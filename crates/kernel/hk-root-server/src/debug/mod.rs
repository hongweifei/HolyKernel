//! 用户异常现场与内核符号表。
//!
//! 平台提供符号表字节、异常现场与用户内存访问。符号解析与格式化在
//! `symbols`，异常回溯与现场文本在 `fault`；异常内容不驻留内核，由订阅者
//! （用户态调试服务）保存并按地址符号化。

mod fault;
mod symbols;

pub use fault::{FaultContext, Memory, format_record};
pub use symbols::{install_symbols, lookup, symbolize, symbolize_list};
