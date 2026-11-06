//! Checkpoint Server：保存、恢复与列出 Native Task 检查点。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::checkpoint::{
    CHECKPOINT_LIST, CHECKPOINT_RESTORE, CHECKPOINT_RESULT, CHECKPOINT_SAVE,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::holy::{DocId, DocumentKind, HolyClient, decode_entries};
#[cfg(target_os = "none")]
use hk_server_api::root::{
    ROOT_OP_CHECKPOINT, ROOT_OP_CHECKPOINT_READ, ROOT_OP_CHECKPOINT_WRITE,
    ROOT_OP_RESTORE_CHECKPOINT, call, call_data,
};
#[cfg(target_os = "none")]
use hk_server_api::{recv_data, send_data, write};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;

/// 快照分块大小。
#[cfg(target_os = "none")]
const CHUNK: usize = 16 * 1024;

/// 清单魔数。
#[cfg(target_os = "none")]
const MANIFEST: &str = "HKCKP1";

/// 存放快照的 Holy 集合。
#[cfg(target_os = "none")]
const COLLECTION: &str = "Checkpoints";

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-checkpoint: ready\n");
    let mut cmdline = [0_u8; 128];
    let disk = hk_server_api::boot_cmdline(&mut cmdline).is_some_and(|line| {
        line.contains("checkpoint.disk") || line.contains("checkpoint-srv-test")
    });
    let mut names: Vec<String> = Vec::new();
    let mut payload = [0_u8; 128];
    loop {
        let (label, _word, sender, length) = recv_data(endpoint, &mut payload);
        let name = core::str::from_utf8(&payload[..length.min(payload.len())])
            .unwrap_or("")
            .trim();
        let text = match label {
            CHECKPOINT_SAVE => {
                let text = if disk {
                    save(endpoint, name)
                } else {
                    root_text(ROOT_OP_CHECKPOINT, name)
                };
                if text.starts_with("已检查点") && !name.is_empty() {
                    names.retain(|saved| saved != name);
                    names.push(String::from(name));
                }
                text
            }
            CHECKPOINT_RESTORE => {
                if disk {
                    restore(endpoint, name)
                } else {
                    root_text(ROOT_OP_RESTORE_CHECKPOINT, name)
                }
            }
            CHECKPOINT_LIST => {
                if disk {
                    list(endpoint, &mut names)
                } else {
                    names_text(&names)
                }
            }
            _ => String::from("未知检查点操作\n"),
        };
        let _ = send_data(sender, CHECKPOINT_RESULT, text.as_bytes());
    }
}

/// 保存检查点并把镜像写入 `H:/Checkpoints`。
#[cfg(target_os = "none")]
fn save(endpoint: u64, name: &str) -> String {
    let mut header = [0_u8; 256];
    let length = match call(ROOT_OP_CHECKPOINT, name, &mut header) {
        Ok(length) => length,
        Err(_) => return String::from("检查点失败：Root 控制失败\n"),
    };
    let text = String::from_utf8_lossy(&header[..length]).into_owned();
    let Some(size) = text
        .trim_end()
        .rsplit(' ')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return text;
    };

    let mut client = HolyClient::new(endpoint);
    let Some(collection) = collection(&mut client) else {
        return format!("{text}（HolyFS 不可用，仅在内存）");
    };
    if size == 0 {
        return String::from("保存失败：快照为空\n");
    }

    let parts = size.div_ceil(CHUNK as u64) as usize;
    let mut chunk = [0_u8; CHUNK];
    for part in 0..parts {
        let part_name = format!("{name}.part{part}");
        let Some(document) = document(&mut client, collection, &part_name) else {
            return format!("{text}（无法创建 H:/Checkpoints/{part_name}）");
        };
        let offset = part as u64 * CHUNK as u64;
        let argument = format!("{name} {offset}");
        let read = match call(ROOT_OP_CHECKPOINT_READ, &argument, &mut chunk) {
            Ok(read) => read,
            Err(_) => return String::from("保存失败：读取快照\n"),
        };
        if read == 0 {
            return String::from("保存失败：快照不完整\n");
        }
        if let Err(status) = client.write_id(document, 0, &chunk[..read]) {
            return format!("保存失败：写入 HolyFS（{status}）part={part}\n");
        }
    }
    if client.sync().is_err() {
        return String::from("保存失败：同步 HolyFS\n");
    }

    let Some(manifest_document) = document(&mut client, collection, name) else {
        return format!("{text}（无法创建 H:/Checkpoints/{name}）");
    };
    let manifest = format!("{MANIFEST}\n{size}\n{parts}\n");
    if let Err(status) = client.write_id(manifest_document, 0, manifest.as_bytes()) {
        return format!("保存失败：写入清单（{status}）\n");
    }
    let _ = client.truncate_id(manifest_document, manifest.len() as u64);
    if client.sync().is_err() {
        return String::from("保存失败：同步清单\n");
    }
    format!("已检查点：{name} {size}（H:/Checkpoints/{name}）\n")
}

/// 从 `H:/Checkpoints` 读取清单与分块并交给 Root 重建；没有落盘时回退内存快照。
#[cfg(target_os = "none")]
fn restore(endpoint: u64, name: &str) -> String {
    let mut client = HolyClient::new(endpoint);
    let Some(collection) = collection(&mut client) else {
        return root_text(ROOT_OP_RESTORE_CHECKPOINT, name);
    };
    let Ok(manifest) = client.resolve(collection, name) else {
        return root_text(ROOT_OP_RESTORE_CHECKPOINT, name);
    };
    let mut header = [0_u8; 128];
    let Ok(length) = client.read_id(manifest, 0, &mut header) else {
        return String::from("恢复失败：读取清单\n");
    };
    let text = core::str::from_utf8(&header[..length]).unwrap_or("");
    let mut lines = text.lines();
    if lines.next() != Some(MANIFEST) {
        return root_text(ROOT_OP_RESTORE_CHECKPOINT, name);
    }
    let Some(total) = lines.next().and_then(|value| value.parse::<u64>().ok()) else {
        return String::from("恢复失败：清单损坏\n");
    };
    let Some(parts) = lines.next().and_then(|value| value.parse::<usize>().ok()) else {
        return String::from("恢复失败：清单损坏\n");
    };
    if total == 0 || parts == 0 {
        return String::from("恢复失败：快照为空\n");
    }

    let mut chunk = [0_u8; CHUNK];
    let mut result = [0_u8; 256];
    let mut sent = 0_u64;
    let mut last = String::from("恢复失败：快照不完整\n");
    for part in 0..parts {
        let part_name = format!("{name}.part{part}");
        let Ok(document) = client.resolve(collection, &part_name) else {
            return String::from("恢复失败：缺少分块\n");
        };
        let Ok(view) = client.document_id(document) else {
            return String::from("恢复失败：读取分块\n");
        };
        let mut offset = 0_u64;
        while offset < view.size {
            let length = match client.read_id(document, offset, &mut chunk) {
                Ok(length) => length,
                Err(status) => {
                    return format!(
                        "恢复失败：读取 HolyFS（{status}）part={part} offset={offset}\n"
                    );
                }
            };
            if length == 0 {
                break;
            }
            let argument = format!("{name} {total}");
            let text_length = match call_data(
                ROOT_OP_CHECKPOINT_WRITE,
                &argument,
                &chunk[..length],
                &mut result,
            ) {
                Ok(length) => length,
                Err(_) => return String::from("恢复失败：Root 控制失败\n"),
            };
            last = String::from_utf8_lossy(&result[..text_length]).into_owned();
            offset += length as u64;
            sent += length as u64;
        }
    }
    if sent != total {
        return String::from("恢复失败：分块不完整\n");
    }
    last
}

/// 列出内存与 `H:/Checkpoints` 中的检查点。
#[cfg(target_os = "none")]
fn list(endpoint: u64, names: &mut Vec<String>) -> String {
    let mut client = HolyClient::new(endpoint);
    if let Some(collection) = collection(&mut client)
        && let Ok(bytes) = client.enumerate_id(collection)
    {
        for entry in decode_entries(bytes) {
            if entry.name.is_empty() || entry.name.contains(".part") {
                continue;
            }
            if !names.iter().any(|name| name == entry.name) {
                names.push(String::from(entry.name));
            }
        }
    }
    names.sort();
    names_text(names)
}

#[cfg(target_os = "none")]
fn names_text(names: &[String]) -> String {
    if names.is_empty() {
        return String::from("无检查点\n");
    }
    let mut text = String::from("检查点\n");
    for name in names {
        text.push_str(name);
        text.push('\n');
    }
    text
}

/// 解析或创建 `H:/Checkpoints` 集合。
#[cfg(target_os = "none")]
fn collection(client: &mut HolyClient) -> Option<DocId> {
    for _ in 0..100 {
        if let Ok(root) = client.root() {
            if let Ok(collection) = client.resolve(root, COLLECTION) {
                return Some(collection);
            }
            return client
                .create(root, COLLECTION, DocumentKind::Collection)
                .ok();
        }
        let _ = hk_server_api::yield_now();
    }
    None
}

/// 解析或创建集合中的快照文档。
#[cfg(target_os = "none")]
fn document(client: &mut HolyClient, collection: DocId, name: &str) -> Option<DocId> {
    if let Ok(document) = client.resolve(collection, name) {
        return Some(document);
    }
    client.create(collection, name, DocumentKind::Content).ok()
}

#[cfg(target_os = "none")]
fn root_text(op: u64, name: &str) -> String {
    let mut buffer = [0_u8; 512];
    match call(op, name, &mut buffer) {
        Ok(length) => String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned(),
        Err(code) => format!("Root 控制失败：{code}\n"),
    }
}

hk_server_api::server_boilerplate!();
