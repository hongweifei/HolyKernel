//! `@exec` 命令：允许列表内的安全命令，返回待渲染的输出行。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_server_api::{civil_time, ticks, write};

use crate::document::DocSet;

/// 执行命令并返回输出行。
pub(crate) fn exec(set: &DocSet, command: &str) -> Vec<String> {
    write(b"hk-server-holydoc: exec ");
    write(command.as_bytes());
    write(b"\n");
    let mut parts = command.split_whitespace();
    match parts.next() {
        Some("version") => alloc::vec![String::from("HolyDoc 0.1 · HolyKernel M6")],
        Some("ticks") => alloc::vec![format!("系统 tick：{}", ticks())],
        Some("time") => match civil_time() {
            Some(time) => alloc::vec![format!(
                "民用时间：{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                time.year, time.month, time.day, time.hour, time.minute, time.second
            )],
            None => alloc::vec![String::from("民用时间：不可用")],
        },
        Some("echo") => alloc::vec![parts.collect::<Vec<_>>().join(" ")],
        Some("documents") | Some("docs") => set
            .entries()
            .iter()
            .map(|entry| format!("{} — {}", entry.id, entry.title))
            .collect(),
        _ => alloc::vec![String::from(
            "未知命令；支持 version / time / ticks / echo / documents"
        )],
    }
}
