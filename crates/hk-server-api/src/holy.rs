//! Holy 卷原生文档协议：按 DocPath 枚举集合、整文档读取与修订视图。
//!
//! 协议不携带 offset/fd：调用方按文档身份取整篇内容。POSIX 路径与偏移由
//! VFS 适配层负责。

use crate::vfs::{
    STATUS_ERROR, STATUS_INVALID, STATUS_IO, STATUS_OK, decode_response, encode_request,
};
use crate::{recv_data_from, send_data, service_endpoint};

/// 请求标签。
pub const HOLY_REQUEST: u64 = 0x800;
/// 响应标签。
pub const HOLY_RESPONSE: u64 = 0x801;

/// 枚举集合。
pub const OP_ENUMERATE: u8 = 1;
/// 整文档读取。
pub const OP_READ: u8 = 2;
/// 文档视图。
pub const OP_DOCUMENT: u8 = 3;
/// 修订链。
pub const OP_REVISIONS: u8 = 4;

/// 内容文档。
pub const KIND_CONTENT: u8 = 0;
/// 集合文档。
pub const KIND_COLLECTION: u8 = 1;

/// 当前承载卷的服务名。
pub const DRIVE_SERVICE: &str = "hk-server-vfs";

/// 请求缓冲上限。
const REQUEST: usize = 1024;
/// 响应缓冲上限。
const RESPONSE: usize = 64 * 1024;
/// 集合项头：kind + DocId + 名字长度。
const ENTRY_HEADER: usize = 1 + 16 + 1;
/// 文档视图长度。
const DOCUMENT_LEN: usize = 1 + 8 + 8 + 8 + 8;
/// 修订视图长度。
const REVISION_LEN: usize = 8 + 8 + 8 + 8 + 4;

/// 集合项视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// 文档类型。
    pub kind: u8,
    /// 文档身份。
    pub document: u128,
    /// 集合内名字。
    pub name: &'a str,
}

/// 文档视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentView {
    /// 文档类型。
    pub kind: u8,
    /// head 修订。
    pub head: u64,
    /// head 的父修订；0 表示根。
    pub parent: u64,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间。
    pub moment: u64,
}

/// 修订视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionView {
    /// 修订身份。
    pub id: u64,
    /// 父修订；0 表示根。
    pub parent: u64,
    /// 内容字节数。
    pub size: u64,
    /// 民用时间。
    pub moment: u64,
    /// 修订标志。
    pub flags: u32,
}

/// 编码一个集合项，返回写入结束位置。
pub fn encode_entry(
    out: &mut [u8],
    offset: usize,
    kind: u8,
    document: u128,
    name: &str,
) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() > u8::MAX as usize {
        return None;
    }
    let end = offset.checked_add(ENTRY_HEADER)?.checked_add(bytes.len())?;
    if end > out.len() {
        return None;
    }
    out[offset] = kind;
    out[offset + 1..offset + 17].copy_from_slice(&document.to_le_bytes());
    out[offset + 17] = bytes.len() as u8;
    out[offset + 18..end].copy_from_slice(bytes);
    Some(end)
}

/// 集合项迭代器。
pub struct Entries<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + ENTRY_HEADER > self.bytes.len() {
            return None;
        }
        let kind = self.bytes[start];
        let document = u128::from_le_bytes(self.bytes[start + 1..start + 17].try_into().ok()?);
        let length = self.bytes[start + 17] as usize;
        let name_start = start + ENTRY_HEADER;
        let end = name_start.checked_add(length)?;
        if end > self.bytes.len() {
            return None;
        }
        let name = core::str::from_utf8(&self.bytes[name_start..end]).ok()?;
        self.offset = end;
        Some(Entry {
            kind,
            document,
            name,
        })
    }
}

/// 解析集合项序列。
pub fn decode_entries(bytes: &[u8]) -> Entries<'_> {
    Entries { bytes, offset: 0 }
}

/// 解析文档视图。
pub fn decode_document(bytes: &[u8]) -> Option<DocumentView> {
    if bytes.len() < DOCUMENT_LEN {
        return None;
    }
    Some(DocumentView {
        kind: bytes[0],
        head: u64::from_le_bytes(bytes[1..9].try_into().ok()?),
        parent: u64::from_le_bytes(bytes[9..17].try_into().ok()?),
        size: u64::from_le_bytes(bytes[17..25].try_into().ok()?),
        moment: u64::from_le_bytes(bytes[25..33].try_into().ok()?),
    })
}

/// 编码文档视图。
pub fn encode_document(out: &mut [u8], view: DocumentView) -> Option<usize> {
    if out.len() < DOCUMENT_LEN {
        return None;
    }
    out[0] = view.kind;
    out[1..9].copy_from_slice(&view.head.to_le_bytes());
    out[9..17].copy_from_slice(&view.parent.to_le_bytes());
    out[17..25].copy_from_slice(&view.size.to_le_bytes());
    out[25..33].copy_from_slice(&view.moment.to_le_bytes());
    Some(DOCUMENT_LEN)
}

/// 在 `offset` 处编码一条修订视图，返回写入结束位置。
pub fn encode_revision(out: &mut [u8], offset: usize, view: RevisionView) -> Option<usize> {
    let end = offset.checked_add(REVISION_LEN)?;
    if end > out.len() {
        return None;
    }
    out[offset..offset + 8].copy_from_slice(&view.id.to_le_bytes());
    out[offset + 8..offset + 16].copy_from_slice(&view.parent.to_le_bytes());
    out[offset + 16..offset + 24].copy_from_slice(&view.size.to_le_bytes());
    out[offset + 24..offset + 32].copy_from_slice(&view.moment.to_le_bytes());
    out[offset + 32..offset + 36].copy_from_slice(&view.flags.to_le_bytes());
    Some(end)
}

/// 修订视图迭代器。
pub struct Revisions<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for Revisions<'_> {
    type Item = RevisionView;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + REVISION_LEN > self.bytes.len() {
            return None;
        }
        let view = RevisionView {
            id: u64::from_le_bytes(self.bytes[start..start + 8].try_into().ok()?),
            parent: u64::from_le_bytes(self.bytes[start + 8..start + 16].try_into().ok()?),
            size: u64::from_le_bytes(self.bytes[start + 16..start + 24].try_into().ok()?),
            moment: u64::from_le_bytes(self.bytes[start + 24..start + 32].try_into().ok()?),
            flags: u32::from_le_bytes(self.bytes[start + 32..start + 36].try_into().ok()?),
        };
        self.offset = start + REVISION_LEN;
        Some(view)
    }
}

/// 解析修订序列。
pub fn decode_revisions(bytes: &[u8]) -> Revisions<'_> {
    Revisions { bytes, offset: 0 }
}

/// 固定缓冲 Holy 文档客户端。
pub struct HolyClient {
    own_endpoint: u64,
    endpoint: u64,
    request: [u8; REQUEST],
    response: [u8; RESPONSE],
}

impl HolyClient {
    /// 构造客户端；`own_endpoint` 是本任务端点。
    pub const fn new(own_endpoint: u64) -> Self {
        Self {
            own_endpoint,
            endpoint: 0,
            request: [0; REQUEST],
            response: [0; RESPONSE],
        }
    }

    /// 枚举集合文档，返回编码后的集合项字节数。
    pub fn enumerate(&mut self, path: &str) -> Result<&[u8], u8> {
        let length = self.exchange(OP_ENUMERATE, path)?;
        Ok(&self.response[3..3 + length])
    }

    /// 整文档读取，返回内容字节数。
    pub fn read(&mut self, path: &str, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_READ, path)?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 查询文档视图。
    pub fn document(&mut self, path: &str) -> Result<DocumentView, u8> {
        let length = self.exchange(OP_DOCUMENT, path)?;
        decode_document(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 枚举修订链，返回编码后的修订字节数。
    pub fn revisions(&mut self, path: &str) -> Result<&[u8], u8> {
        let length = self.exchange(OP_REVISIONS, path)?;
        Ok(&self.response[3..3 + length])
    }

    fn exchange(&mut self, op: u8, path: &str) -> Result<usize, u8> {
        let Some(length) = encode_request(&mut self.request, op, path, 0, 0, &[]) else {
            return Err(STATUS_INVALID);
        };
        let endpoint = service_endpoint(DRIVE_SERVICE);
        if endpoint == 0 {
            return Err(STATUS_IO);
        }
        self.endpoint = endpoint;
        if send_data(self.endpoint, HOLY_REQUEST, &self.request[..length]) == u64::MAX {
            return Err(STATUS_IO);
        }
        let (label, _word, _sender, length) =
            recv_data_from(self.own_endpoint, self.endpoint, &mut self.response);
        if label != HOLY_RESPONSE {
            return Err(STATUS_IO);
        }
        if length < 3 {
            return Err(STATUS_ERROR);
        }
        let response = decode_response(&self.response[..length]).ok_or(STATUS_INVALID)?;
        if response.status != STATUS_OK {
            return Err(response.status);
        }
        Ok(response.data.len())
    }
}
