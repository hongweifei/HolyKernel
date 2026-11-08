//! 用户态 Shell 的标签命令。
//!
//! 标签属于 H: 卷，命令通过 Holy 协议直接访问 VFS 服务；核心命令表保持
//! 中立，宿主扩展负责本端能力。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_proto::holy::{
    DocId, STATUS_NOT_FOUND, STATUS_PERMISSION, decode_doc_ids, decode_tags, decode_u32,
};
use hk_proto::vfs::{STATUS_EXISTS, STATUS_INVALID, STATUS_IO};
#[cfg(target_os = "none")]
use hk_proto_client::HolyClient;

/// 处理标签扩展命令；不是标签命令时返回 `None`。
pub(crate) fn extension(endpoint: u64, name: &str, args: &str) -> Option<String> {
    match name {
        "tags" => Some(list(endpoint)),
        "tag" => Some(add(endpoint, args)),
        "untag" => Some(remove(endpoint, args)),
        "tags-of" => Some(tags_of(endpoint, args)),
        "tag-members" => Some(members(endpoint, args)),
        "tag-create" => Some(create(endpoint, args)),
        "tag-drop" => Some(drop_tag(endpoint, args)),
        _ => None,
    }
}

fn client(endpoint: u64) -> HolyClient {
    HolyClient::new(endpoint)
}

/// 枚举标签及其成员数。
fn list(endpoint: u64) -> String {
    let mut client = client(endpoint);
    let Ok(bytes) = client.tags() else {
        return String::from("标签不可用：VFS 未挂载 H: 卷\n");
    };
    let tags: Vec<(u32, String)> = decode_tags(bytes)
        .map(|tag| (tag.id, String::from(tag.name)))
        .collect();
    if tags.is_empty() {
        return String::from("无标签\n");
    }
    let mut output = format!("标签（{} 项）\n编号\t名字\t成员\n", tags.len());
    for (id, name) in tags {
        let count = client
            .tag_members(id)
            .map(|bytes| bytes.len() / 16)
            .unwrap_or(0);
        output.push_str(&format!("{id}\t{name}\t{count}\n"));
    }
    output
}

/// 创建标签。
fn create(endpoint: u64, args: &str) -> String {
    let name = args.trim();
    if name.is_empty() {
        return String::from("用法：tag-create <名字>\n");
    }
    match client(endpoint).create_tag(name) {
        Ok(id) => format!("已创建标签 {id}：{name}\n"),
        Err(status) => failure("创建标签", status),
    }
}

/// 删除标签。
fn drop_tag(endpoint: u64, args: &str) -> String {
    let Some(tag) = parse_tag(args) else {
        return String::from("用法：tag-drop <编号>\n");
    };
    match client(endpoint).drop_tag(tag) {
        Ok(()) => format!("已删除标签 {tag}\n"),
        Err(status) => failure("删除标签", status),
    }
}

/// 把文档加入标签。
fn add(endpoint: u64, args: &str) -> String {
    let Some((path, tag)) = parse_document_tag(args) else {
        return String::from("用法：tag <文档路径> <编号>\n");
    };
    let mut client = client(endpoint);
    let document = match resolve(&mut client, path) {
        Ok(document) => document,
        Err(status) => return failure("解析文档", status),
    };
    match client.tag(document, tag) {
        Ok(()) => format!("已把 {path} 加入标签 {tag}\n"),
        Err(status) => failure("加入标签", status),
    }
}

/// 把文档从标签移除。
fn remove(endpoint: u64, args: &str) -> String {
    let Some((path, tag)) = parse_document_tag(args) else {
        return String::from("用法：untag <文档路径> <编号>\n");
    };
    let mut client = client(endpoint);
    let document = match resolve(&mut client, path) {
        Ok(document) => document,
        Err(status) => return failure("解析文档", status),
    };
    match client.untag(document, tag) {
        Ok(()) => format!("已把 {path} 从标签 {tag} 移除\n"),
        Err(status) => failure("移除标签", status),
    }
}

/// 枚举文档所属标签。
fn tags_of(endpoint: u64, args: &str) -> String {
    let path = args.trim();
    if path.is_empty() {
        return String::from("用法：tags-of <文档路径>\n");
    }
    let mut client = client(endpoint);
    let document = match resolve(&mut client, path) {
        Ok(document) => document,
        Err(status) => return failure("解析文档", status),
    };
    let ids: Vec<u32> = match client.tags_of(document) {
        Ok(bytes) => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .filter_map(|chunk| decode_u32(chunk))
            .collect(),
        Err(status) => return failure("查询文档标签", status),
    };
    if ids.is_empty() {
        return format!("{path}：无标签\n");
    }
    let names: Vec<(u32, String)> = match client.tags() {
        Ok(bytes) => decode_tags(bytes)
            .map(|tag| (tag.id, String::from(tag.name)))
            .collect(),
        Err(_) => Vec::new(),
    };
    let mut output = format!("{path}：");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            output.push('、');
        }
        match names.iter().find(|(tag, _)| tag == id) {
            Some((_, name)) => output.push_str(&format!("{id}({name})")),
            None => output.push_str(&format!("{id}")),
        }
    }
    output.push('\n');
    output
}

/// 枚举标签成员。
fn members(endpoint: u64, args: &str) -> String {
    let Some(tag) = parse_tag(args) else {
        return String::from("用法：tag-members <编号>\n");
    };
    match client(endpoint).tag_members(tag) {
        Ok(bytes) => {
            let mut output = String::new();
            for document in decode_doc_ids(bytes) {
                output.push_str(&format!("{document}\n"));
            }
            if output.is_empty() {
                String::from("标签无成员\n")
            } else {
                output
            }
        }
        Err(status) => failure("枚举标签成员", status),
    }
}

/// 按 DocPath 解析文档身份，忽略盘符与空段。
fn resolve(client: &mut HolyClient, path: &str) -> Result<DocId, u8> {
    let mut document = client.root()?;
    for (index, component) in path.split('/').enumerate() {
        if component.is_empty() || component == "." {
            continue;
        }
        if index == 0 && component.ends_with(':') {
            continue;
        }
        document = client.resolve(document, component)?;
    }
    Ok(document)
}

fn parse_tag(args: &str) -> Option<u32> {
    args.trim().parse().ok()
}

fn parse_document_tag(args: &str) -> Option<(&str, u32)> {
    let mut parts = args.split_whitespace();
    let path = parts.next()?;
    let tag = parts.next()?.parse().ok()?;
    Some((path, tag))
}

fn failure(action: &str, status: u8) -> String {
    format!("{action}失败：{}\n", status_text(status))
}

fn status_text(status: u8) -> &'static str {
    match status {
        STATUS_NOT_FOUND => "未找到",
        STATUS_EXISTS => "已存在",
        STATUS_INVALID => "无效参数",
        STATUS_PERMISSION => "权限不足",
        STATUS_IO => "I/O 错误",
        _ => "未知错误",
    }
}
