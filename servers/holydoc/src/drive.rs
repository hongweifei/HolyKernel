//! H: 卷加载：集合递归、文档读取与标签索引。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use hk_server_api::holy::{
    DocId, HolyClient, KIND_COLLECTION, RIGHT_READ, decode_doc_ids, decode_entries, decode_tags,
};
use hk_server_api::{service_endpoint, write, write_u64};

use crate::document::{DocSet, DocTag};
use crate::markup::{doc_name, title_of};

/// H: 卷根集合。
pub(crate) const DRIVE_ROOT: &str = "H:/";

/// 集合递归深度上限。
const MAX_DEPTH: usize = 4;

/// 单页文档读取上限。
const DOC_MAX: usize = 48 * 1024;

/// 一次驱动加载的结果。
pub(crate) struct Loaded {
    /// 加载的文档页数。
    pub(crate) documents: usize,
    /// 加载的标签数。
    pub(crate) tags: usize,
}

/// 从 H: 卷递归加载集合、文档与标签。
pub(crate) fn load(set: &mut DocSet, endpoint: u64) -> Loaded {
    if service_endpoint("hk-server-holyfs") == 0 {
        write(b"hk-server-holydoc: no drive\n");
        return Loaded {
            documents: 0,
            tags: 0,
        };
    }
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(root) = client.root() else {
        write(b"hk-server-holydoc: no root\n");
        return Loaded {
            documents: 0,
            tags: 0,
        };
    };
    let mut documents = 0;
    load_collection(&mut client, root, DRIVE_ROOT, 0, set, &mut documents);
    let tags = load_tags(&mut client, set);
    Loaded { documents, tags }
}

/// 读取标签表与成员关系，写入文档集。
pub(crate) fn load_tags(client: &mut HolyClient, set: &mut DocSet) -> usize {
    let Ok(bytes) = client.tags() else {
        return 0;
    };
    let tags: Vec<(u32, String)> = decode_tags(bytes)
        .map(|tag| (tag.id, String::from(tag.name)))
        .collect();
    let mut result = Vec::with_capacity(tags.len());
    for (id, name) in tags {
        let members = match client.tag_members(id) {
            Ok(bytes) => decode_doc_ids(bytes)
                .map(|document| document.get())
                .collect(),
            Err(_) => Vec::new(),
        };
        result.push(DocTag { id, name, members });
    }
    let count = result.len();
    set.set_tags(result);
    count
}

/// 加载一个集合：文档整篇读入，子集合生成成员页并继续递归。
fn load_collection(
    client: &mut HolyClient,
    collection: DocId,
    path: &str,
    depth: usize,
    set: &mut DocSet,
    count: &mut usize,
) -> Vec<(u8, String, String)> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let entries: Vec<(u8, u128, String)> = match client.enumerate_id(collection) {
        Ok(bytes) => decode_entries(bytes)
            .map(|entry| (entry.kind, entry.document, String::from(entry.name)))
            .collect(),
        Err(status) => {
            write(b"hk-server-holydoc: drive ");
            write(path.as_bytes());
            write(b" status ");
            write_u64(status as u64);
            write(b"\n");
            return Vec::new();
        }
    };
    let mut members = Vec::new();
    for (kind, document, name) in entries {
        let child = join(path, &name);
        members.push((kind, name.clone(), child.clone()));
        let document = DocId::new(document);
        if kind == KIND_COLLECTION {
            let children = load_collection(client, document, &child, depth + 1, set, count);
            let mut body =
                format!("@title(\"{name}\")\n这是 H: 卷中的一个集合。\n\n@heading(2, \"成员\")\n");
            for (kind, label, path) in &children {
                if *kind == KIND_COLLECTION {
                    body.push_str(&format!("@link(\"{path}\", \"{label}/\")\n"));
                } else {
                    body.push_str(&format!("@link(\"{path}\", \"{label}\")\n"));
                }
            }
            if children.is_empty() {
                body.push_str("集合为空。\n");
            }
            set.add(child.clone(), name.clone(), body, Some(document.get()));
            continue;
        }
        let mut buffer = vec![0_u8; DOC_MAX];
        let result = match client.acquire(document, RIGHT_READ) {
            Ok(token) => {
                let result = client.read_token(token, 0, &mut buffer);
                let _ = client.release(token);
                result
            }
            Err(status) => Err(status),
        };
        match result {
            Ok(length) => {
                let body = String::from_utf8_lossy(&buffer[..length]).into_owned();
                let title = title_of(&body).unwrap_or_else(|| doc_name(&child));
                if set.add(child.clone(), title, body, Some(document.get())) {
                    *count += 1;
                    write(b"hk-server-holydoc: document ");
                    write(child.as_bytes());
                    write(b"\n");
                }
            }
            Err(status) => {
                write(b"hk-server-holydoc: read ");
                write(child.as_bytes());
                write(b" status ");
                write_u64(status as u64);
                write(b"\n");
            }
        }
    }
    members
}

/// 拼接集合成员的 DocPath。
fn join(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}
