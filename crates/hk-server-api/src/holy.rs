//! Holy 卷原生文档协议：DocPath、DocId、偏移读写与句柄令牌。
//!
//! 路径操作保留给兼容客户端；原生客户端使用 `DocId` 与令牌，按文档身份
//! 和权利访问，不依赖 fd 或 POSIX 路径语义。

use core::fmt;

use crate::vfs::{
    STATUS_ERROR, STATUS_INVALID, STATUS_IO, STATUS_OK, decode_response, encode_request,
};
pub use crate::vfs::{STATUS_NOT_FOUND, STATUS_PERMISSION};
use crate::{recv_data_from, send_data, service_endpoint};

/// 线上文档身份；与原生 `DocId` 同构，不依赖分配。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(u128);

impl DocId {
    /// 根文档身份。
    pub const ROOT: Self = Self(1);

    /// 构造文档身份。
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// 返回原始值。
    pub const fn get(self) -> u128 {
        self.0
    }

    /// 编码为小端字节。
    pub const fn to_le_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }

    /// 从小端字节解码。
    pub const fn from_le_bytes(bytes: [u8; 16]) -> Self {
        Self(u128::from_le_bytes(bytes))
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:032x}", self.0)
    }
}

/// 线上修订身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevisionId(u64);

impl RevisionId {
    /// 根集合修订。
    pub const ROOT: Self = Self(1);

    /// 构造修订身份。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回原始值。
    pub const fn get(self) -> u64 {
        self.0
    }

    /// 编码为小端字节。
    pub const fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    /// 从小端字节解码。
    pub const fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self(u64::from_le_bytes(bytes))
    }
}

impl fmt::Display for RevisionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// 文档类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    /// 内容文档。
    Content,
    /// 集合文档。
    Collection,
}

/// 请求标签。
pub const HOLY_REQUEST: u64 = 0x800;
/// 响应标签。
pub const HOLY_RESPONSE: u64 = 0x801;

/// 按 DocPath 枚举集合。
pub const OP_ENUMERATE: u8 = 1;
/// 按 DocPath 整文档读取。
pub const OP_READ: u8 = 2;
/// 按 DocPath 查询文档视图。
pub const OP_DOCUMENT: u8 = 3;
/// 按 DocPath 枚举修订链。
pub const OP_REVISIONS: u8 = 4;
/// 读取根文档身份。
pub const OP_ROOT: u8 = 5;
/// 在集合中按名字解析。
pub const OP_RESOLVE: u8 = 6;
/// 在集合中创建文档。
pub const OP_CREATE: u8 = 7;
/// 移除集合项。
pub const OP_UNBIND: u8 = 8;
/// 按 DocId 枚举集合。
pub const OP_ENUMERATE_ID: u8 = 9;
/// 按 DocId 查询视图。
pub const OP_DOCUMENT_ID: u8 = 10;
/// 按 DocId 与偏移读取。
pub const OP_READ_ID: u8 = 11;
/// 按 DocId 与偏移写入。
pub const OP_WRITE_ID: u8 = 12;
/// 按 DocId 调整大小。
pub const OP_TRUNCATE_ID: u8 = 13;
/// 按 DocId 枚举修订链。
pub const OP_REVISIONS_ID: u8 = 14;
/// 提交卷写入。
pub const OP_SYNC: u8 = 15;
/// 获取文档句柄令牌。
pub const OP_ACQUIRE: u8 = 16;
/// 释放令牌。
pub const OP_RELEASE: u8 = 17;
/// 通过令牌读取。
pub const OP_READ_TOKEN: u8 = 18;
/// 通过令牌写入。
pub const OP_WRITE_TOKEN: u8 = 19;
/// 通过令牌调整大小。
pub const OP_TRUNCATE_TOKEN: u8 = 20;
/// 通过令牌查询视图。
pub const OP_DOCUMENT_TOKEN: u8 = 21;
/// 收窄令牌权利。
pub const OP_GRANT: u8 = 22;
/// 撤销令牌。
pub const OP_REVOKE: u8 = 23;
/// 把令牌转移到另一个端点。
pub const OP_TRANSFER: u8 = 24;
/// 枚举标签。
pub const OP_TAGS: u8 = 25;
/// 枚举标签成员。
pub const OP_TAG_MEMBERS: u8 = 26;
/// 创建标签。
pub const OP_CREATE_TAG: u8 = 27;
/// 删除标签。
pub const OP_DROP_TAG: u8 = 28;
/// 把文档加入标签。
pub const OP_TAG: u8 = 29;
/// 把文档从标签移除。
pub const OP_UNTAG: u8 = 30;
/// 枚举文档所属标签。
pub const OP_TAGS_OF: u8 = 31;

/// 内容文档。
pub const KIND_CONTENT: u8 = 0;
/// 集合文档。
pub const KIND_COLLECTION: u8 = 1;

/// 查询视图。
pub const RIGHT_INSPECT: u32 = 1 << 0;
/// 读取内容。
pub const RIGHT_READ: u32 = 1 << 1;
/// 写入内容。
pub const RIGHT_WRITE: u32 = 1 << 2;
/// 集合变更。
pub const RIGHT_BIND: u32 = 1 << 3;
/// 管理操作。
pub const RIGHT_ADMIN: u32 = 1 << 4;
/// 转移令牌。
pub const RIGHT_TRANSFER: u32 = 1 << 5;

/// 当前承载卷的服务名。
pub const DRIVE_SERVICE: &str = "hk-server-vfs";

/// 请求缓冲上限。
const REQUEST: usize = 64 * 1024;
/// 响应缓冲上限。
const RESPONSE: usize = 64 * 1024;
/// 集合项头：kind + DocId + 名字长度。
const ENTRY_HEADER: usize = 1 + 16 + 1;
/// 文档视图长度。
const DOCUMENT_LEN: usize = 1 + 8 + 8 + 8 + 8;
/// 修订视图长度。
const REVISION_LEN: usize = 8 + 8 + 8 + 8 + 4;
/// 文档身份编码长度。
const DOC_LEN: usize = 16;
/// 修订身份编码长度。
const REVISION_ID_LEN: usize = 8;
/// 令牌编码长度。
const TOKEN_LEN: usize = 8;
/// 标签头：编号 + 名字长度。
const TAG_HEADER: usize = 4 + 1;

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

/// 解析文档身份。
pub fn decode_doc_id(bytes: &[u8]) -> Option<DocId> {
    Some(DocId::from_le_bytes(bytes.get(..DOC_LEN)?.try_into().ok()?))
}

/// 编码文档身份。
pub fn encode_doc_id(out: &mut [u8], document: DocId) -> Option<usize> {
    if out.len() < DOC_LEN {
        return None;
    }
    out[..DOC_LEN].copy_from_slice(&document.to_le_bytes());
    Some(DOC_LEN)
}

/// 解析修订身份。
pub fn decode_revision_id(bytes: &[u8]) -> Option<RevisionId> {
    Some(RevisionId::from_le_bytes(
        bytes.get(..REVISION_ID_LEN)?.try_into().ok()?,
    ))
}

/// 编码修订身份。
pub fn encode_revision_id(out: &mut [u8], revision: RevisionId) -> Option<usize> {
    if out.len() < REVISION_ID_LEN {
        return None;
    }
    out[..REVISION_ID_LEN].copy_from_slice(&revision.to_le_bytes());
    Some(REVISION_ID_LEN)
}

/// 标签视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagView<'a> {
    /// 标签编号。
    pub id: u32,
    /// 标签名字。
    pub name: &'a str,
}

/// 编码一个标签，返回写入结束位置。
pub fn encode_tag(out: &mut [u8], offset: usize, id: u32, name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() > u8::MAX as usize {
        return None;
    }
    let end = offset.checked_add(TAG_HEADER)?.checked_add(bytes.len())?;
    if end > out.len() {
        return None;
    }
    out[offset..offset + 4].copy_from_slice(&id.to_le_bytes());
    out[offset + 4] = bytes.len() as u8;
    out[offset + TAG_HEADER..end].copy_from_slice(bytes);
    Some(end)
}

/// 标签迭代器。
pub struct Tags<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Tags<'a> {
    type Item = TagView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        if start + TAG_HEADER > self.bytes.len() {
            return None;
        }
        let id = u32::from_le_bytes(self.bytes[start..start + 4].try_into().ok()?);
        let length = self.bytes[start + 4] as usize;
        let name_start = start + TAG_HEADER;
        let end = name_start.checked_add(length)?;
        if end > self.bytes.len() {
            return None;
        }
        let name = core::str::from_utf8(&self.bytes[name_start..end]).ok()?;
        self.offset = end;
        Some(TagView { id, name })
    }
}

/// 解析标签序列。
pub fn decode_tags(bytes: &[u8]) -> Tags<'_> {
    Tags { bytes, offset: 0 }
}

/// 文档身份迭代器。
pub struct DocIds<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for DocIds<'_> {
    type Item = DocId;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        let document = decode_doc_id(self.bytes.get(start..)?)?;
        self.offset = start + DOC_LEN;
        Some(document)
    }
}

/// 解析文档身份序列。
pub fn decode_doc_ids(bytes: &[u8]) -> DocIds<'_> {
    DocIds { bytes, offset: 0 }
}

/// 解析 u32。
pub fn decode_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// 编码 u32。
pub fn encode_u32(out: &mut [u8], value: u32) -> Option<usize> {
    if out.len() < 4 {
        return None;
    }
    out[..4].copy_from_slice(&value.to_le_bytes());
    Some(4)
}

/// 解析令牌。
pub fn decode_token(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..TOKEN_LEN)?.try_into().ok()?))
}

/// 编码令牌。
pub fn encode_token(out: &mut [u8], token: u64) -> Option<usize> {
    if out.len() < TOKEN_LEN {
        return None;
    }
    out[..TOKEN_LEN].copy_from_slice(&token.to_le_bytes());
    Some(TOKEN_LEN)
}

/// 解析 u64。
pub fn decode_u64(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
}

/// 文档类型编码。
pub const fn kind_code(kind: DocumentKind) -> u8 {
    match kind {
        DocumentKind::Content => KIND_CONTENT,
        DocumentKind::Collection => KIND_COLLECTION,
    }
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
        let length = self.exchange(OP_ENUMERATE, path, 0, 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 整文档读取，返回内容字节数。
    pub fn read(&mut self, path: &str, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(OP_READ, path, 0, 0, &[])?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 查询文档视图。
    pub fn document(&mut self, path: &str) -> Result<DocumentView, u8> {
        let length = self.exchange(OP_DOCUMENT, path, 0, 0, &[])?;
        decode_document(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 枚举修订链，返回编码后的修订字节数。
    pub fn revisions(&mut self, path: &str) -> Result<&[u8], u8> {
        let length = self.exchange(OP_REVISIONS, path, 0, 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 读取根集合文档身份。
    pub fn root(&mut self) -> Result<DocId, u8> {
        let length = self.exchange(OP_ROOT, "", 0, 0, &[])?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 在集合中按名字解析文档。
    pub fn resolve(&mut self, collection: DocId, name: &str) -> Result<DocId, u8> {
        let length = self.exchange(OP_RESOLVE, name, 0, 0, &collection.to_le_bytes())?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 在集合中创建文档。
    pub fn create(
        &mut self,
        collection: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, u8> {
        let length = self.exchange(
            OP_CREATE,
            name,
            u64::from(kind_code(kind)),
            0,
            &collection.to_le_bytes(),
        )?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 移除集合项。
    pub fn unbind(&mut self, collection: DocId, name: &str) -> Result<DocId, u8> {
        let length = self.exchange(OP_UNBIND, name, 0, 0, &collection.to_le_bytes())?;
        decode_doc_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 按文档身份枚举集合。
    pub fn enumerate_id(&mut self, document: DocId) -> Result<&[u8], u8> {
        let length = self.exchange(OP_ENUMERATE_ID, "", 0, 0, &document.to_le_bytes())?;
        Ok(&self.response[3..3 + length])
    }

    /// 按文档身份查询视图。
    pub fn document_id(&mut self, document: DocId) -> Result<DocumentView, u8> {
        let length = self.exchange(OP_DOCUMENT_ID, "", 0, 0, &document.to_le_bytes())?;
        decode_document(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 按文档身份与偏移读取。
    pub fn read_id(&mut self, document: DocId, offset: u64, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(
            OP_READ_ID,
            "",
            offset,
            out.len() as u64,
            &document.to_le_bytes(),
        )?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 按文档身份与偏移写入。
    pub fn write_id(
        &mut self,
        document: DocId,
        offset: u64,
        data: &[u8],
    ) -> Result<RevisionId, u8> {
        let length = self.exchange_with_doc(OP_WRITE_ID, offset, 0, document, data)?;
        decode_revision_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 按文档身份调整大小。
    pub fn truncate_id(&mut self, document: DocId, size: u64) -> Result<(), u8> {
        self.exchange(OP_TRUNCATE_ID, "", size, 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 按文档身份枚举修订链。
    pub fn revisions_id(&mut self, document: DocId) -> Result<&[u8], u8> {
        let length = self.exchange(OP_REVISIONS_ID, "", 0, 0, &document.to_le_bytes())?;
        Ok(&self.response[3..3 + length])
    }

    /// 提交卷写入。
    pub fn sync(&mut self) -> Result<(), u8> {
        self.exchange(OP_SYNC, "", 0, 0, &[]).map(|_| ())
    }

    /// 获取文档令牌。
    pub fn acquire(&mut self, document: DocId, rights: u32) -> Result<u64, u8> {
        let length = self.exchange(
            OP_ACQUIRE,
            "",
            u64::from(rights),
            0,
            &document.to_le_bytes(),
        )?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 释放令牌。
    pub fn release(&mut self, token: u64) -> Result<(), u8> {
        self.exchange(OP_RELEASE, "", token, 0, &[]).map(|_| ())
    }

    /// 收窄令牌权利，返回子令牌。
    pub fn grant(&mut self, token: u64, rights: u32) -> Result<u64, u8> {
        let length = self.exchange(OP_GRANT, "", token, u64::from(rights), &[])?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 撤销令牌及其子令牌。
    pub fn revoke(&mut self, token: u64) -> Result<(), u8> {
        self.exchange(OP_REVOKE, "", token, 0, &[]).map(|_| ())
    }

    /// 把令牌转移到目标端点。
    pub fn transfer(&mut self, token: u64, target: u64) -> Result<u64, u8> {
        let length = self.exchange(OP_TRANSFER, "", token, target, &[])?;
        decode_token(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 枚举标签，返回编码后的标签字节数。
    pub fn tags(&mut self) -> Result<&[u8], u8> {
        let length = self.exchange(OP_TAGS, "", 0, 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 枚举标签成员，返回编码后的文档身份字节数。
    pub fn tag_members(&mut self, tag: u32) -> Result<&[u8], u8> {
        let length = self.exchange(OP_TAG_MEMBERS, "", u64::from(tag), 0, &[])?;
        Ok(&self.response[3..3 + length])
    }

    /// 创建标签，返回标签编号。
    pub fn create_tag(&mut self, name: &str) -> Result<u32, u8> {
        let length = self.exchange(OP_CREATE_TAG, name, 0, 0, &[])?;
        decode_u32(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 删除标签。
    pub fn drop_tag(&mut self, tag: u32) -> Result<(), u8> {
        self.exchange(OP_DROP_TAG, "", u64::from(tag), 0, &[])
            .map(|_| ())
    }

    /// 把文档加入标签。
    pub fn tag(&mut self, document: DocId, tag: u32) -> Result<(), u8> {
        self.exchange(OP_TAG, "", u64::from(tag), 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 把文档从标签移除。
    pub fn untag(&mut self, document: DocId, tag: u32) -> Result<(), u8> {
        self.exchange(OP_UNTAG, "", u64::from(tag), 0, &document.to_le_bytes())
            .map(|_| ())
    }

    /// 枚举文档所属标签，返回编码后的标签编号字节数。
    pub fn tags_of(&mut self, document: DocId) -> Result<&[u8], u8> {
        let length = self.exchange(OP_TAGS_OF, "", 0, 0, &document.to_le_bytes())?;
        Ok(&self.response[3..3 + length])
    }

    /// 通过令牌读取。
    pub fn read_token(&mut self, token: u64, offset: u64, out: &mut [u8]) -> Result<usize, u8> {
        let length = self.exchange(
            OP_READ_TOKEN,
            "",
            token,
            offset,
            &(out.len() as u64).to_le_bytes(),
        )?;
        let copy = length.min(out.len());
        out[..copy].copy_from_slice(&self.response[3..3 + copy]);
        Ok(length)
    }

    /// 通过令牌写入。
    pub fn write_token(&mut self, token: u64, offset: u64, data: &[u8]) -> Result<RevisionId, u8> {
        let length = self.exchange(OP_WRITE_TOKEN, "", token, offset, data)?;
        decode_revision_id(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    /// 通过令牌调整大小。
    pub fn truncate_token(&mut self, token: u64, size: u64) -> Result<(), u8> {
        self.exchange(OP_TRUNCATE_TOKEN, "", token, size, &[])
            .map(|_| ())
    }

    /// 通过令牌查询视图。
    pub fn document_token(&mut self, token: u64) -> Result<DocumentView, u8> {
        let length = self.exchange(OP_DOCUMENT_TOKEN, "", token, 0, &[])?;
        decode_document(&self.response[3..3 + length]).ok_or(STATUS_INVALID)
    }

    fn exchange(
        &mut self,
        op: u8,
        path: &str,
        arg0: u64,
        arg1: u64,
        data: &[u8],
    ) -> Result<usize, u8> {
        let Some(length) = encode_request(&mut self.request, op, path, arg0, arg1, data) else {
            return Err(STATUS_INVALID);
        };
        self.submit(length)
    }

    fn exchange_with_doc(
        &mut self,
        op: u8,
        arg0: u64,
        arg1: u64,
        document: DocId,
        payload: &[u8],
    ) -> Result<usize, u8> {
        let header = 1 + 2 + 8 + 8;
        let end = header + DOC_LEN + payload.len();
        if end > self.request.len() {
            return Err(STATUS_INVALID);
        }
        self.request[0] = op;
        self.request[1..3].copy_from_slice(&0_u16.to_le_bytes());
        self.request[3..11].copy_from_slice(&arg0.to_le_bytes());
        self.request[11..19].copy_from_slice(&arg1.to_le_bytes());
        self.request[19..19 + DOC_LEN].copy_from_slice(&document.to_le_bytes());
        self.request[19 + DOC_LEN..end].copy_from_slice(payload);
        self.submit(end)
    }

    fn submit(&mut self, length: usize) -> Result<usize, u8> {
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
