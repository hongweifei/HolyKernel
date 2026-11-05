//! Home / Once 启动文档。
//!
//! `H:/Home` 每次 Shell 启动都会执行，`H:/Once` 只在首次启动执行；Once
//! 完成后写入 `H:/Once.done` 标记。文档中的启动命令使用 `@exec <命令>` 行。

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::holy::{DocumentKind, HolyClient, STATUS_NOT_FOUND};
#[cfg(target_os = "none")]
use hk_server_api::yield_now;

/// 启动文档读取缓冲长度。
#[cfg(target_os = "none")]
const READ_BUFFER: usize = 32 * 1024;

/// 等待 H: 卷挂载的让步次数。
#[cfg(target_os = "none")]
const WAIT_ATTEMPTS: usize = 256;

#[cfg(target_os = "none")]
const HOME: &str = "H:/Home";
#[cfg(target_os = "none")]
const ONCE: &str = "H:/Once";
#[cfg(target_os = "none")]
const ONCE_MARKER: &str = "H:/Once.done";
#[cfg(target_os = "none")]
const ONCE_MARKER_NAME: &str = "Once.done";

/// 解析出的启动命令。
#[cfg(target_os = "none")]
pub(crate) struct Startup {
    pub(crate) commands: Vec<String>,
    pub(crate) mark_once: bool,
}

/// 读取 Home 与待执行的 Once 命令。
#[cfg(target_os = "none")]
pub(crate) fn load(endpoint: u64) -> Startup {
    let mut startup = Startup {
        commands: Vec::new(),
        mark_once: false,
    };
    let mut client = HolyClient::new(endpoint);
    if !wait_drive(&mut client) {
        return startup;
    }
    let mut buffer = [0_u8; READ_BUFFER];
    let mut marker = [0_u8; 1];
    if client.read(ONCE_MARKER, &mut marker).is_err()
        && let Ok(length) = client.read(ONCE, &mut buffer)
    {
        append(&buffer[..length.min(READ_BUFFER)], &mut startup.commands);
        startup.mark_once = true;
    }
    if let Ok(length) = client.read(HOME, &mut buffer) {
        append(&buffer[..length.min(READ_BUFFER)], &mut startup.commands);
    }
    startup
}

/// Once 执行完成后写入标记；失败时下次启动重试。
#[cfg(target_os = "none")]
pub(crate) fn mark_once(endpoint: u64) -> bool {
    let mut client = HolyClient::new(endpoint);
    let Ok(root) = client.root() else {
        return false;
    };
    let document = match client.resolve(root, ONCE_MARKER_NAME) {
        Ok(document) => document,
        Err(STATUS_NOT_FOUND) => {
            let Ok(document) = client.create(root, ONCE_MARKER_NAME, DocumentKind::Content) else {
                return false;
            };
            document
        }
        Err(_) => return false,
    };
    if client.write_id(document, 0, b"done").is_err() {
        return false;
    }
    client.sync().is_ok()
}

/// 从 UTF-8 文档字节中提取 `@exec` 行。
#[cfg(target_os = "none")]
fn append(bytes: &[u8], commands: &mut Vec<String>) {
    if let Ok(text) = core::str::from_utf8(bytes) {
        commands.extend(parse(text));
    }
}

/// 从文档中提取 `@exec` 行。
pub(crate) fn parse(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let command = line.trim().strip_prefix("@exec")?.trim();
            (!command.is_empty()).then(|| String::from(command))
        })
        .collect()
}

#[cfg(target_os = "none")]
fn wait_drive(client: &mut HolyClient) -> bool {
    for _ in 0..WAIT_ATTEMPTS {
        if client.root().is_ok() {
            return true;
        }
        let _ = yield_now();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_exec_lines() {
        let document = "@title Home\n@exec mounts\n# @exec ignored\n  @exec   tags  \n";
        let commands = parse(document);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], "mounts");
        assert_eq!(commands[1], "tags");
    }
}
