//! `@exec` 命令：允许列表内的安全命令，返回待渲染的输出行。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_server_api::{ticks, write};

use crate::document::DOCS;

/// 执行命令并返回输出行。
pub(crate) fn exec(command: &str) -> Vec<String> {
    write(b"hk-server-holydoc: exec ");
    write(command.as_bytes());
    write(b"\n");
    let mut parts = command.split_whitespace();
    match parts.next() {
        Some("version") => alloc::vec![String::from("HolyDoc 0.1 · HolyKernel M6")],
        Some("ticks") => alloc::vec![format!("系统 tick：{}", ticks())],
        Some("echo") => alloc::vec![parts.collect::<Vec<_>>().join(" ")],
        Some("docs") => DOCS
            .iter()
            .map(|doc| format!("{} — {}", doc.id, doc.title))
            .collect(),
        _ => alloc::vec![String::from("未知命令；支持 version / ticks / echo / docs")],
    }
}
