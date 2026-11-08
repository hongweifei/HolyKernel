//! Holy 文档协议的响应构造：状态映射与常见响应编码。
//!
//! 两个 Holy 文档服务共用这套构造，业务侧只负责调用后端并选择响应形状；
//! 新增文件系统错误变体时只需要改 `hk_fs::Error::status` 一处。

use hk_fs::{
    DocId, DocumentInfo, DocumentKind, EntryInfo, Error, RevisionId, RevisionInfo, SnapshotInfo,
    TagInfo,
};

use hk_proto::holy::{
    DocId as WireDocId, DocumentView, DriveView, KIND_COLLECTION, KIND_CONTENT,
    RevisionId as WireRevisionId, RevisionView, VolumeView, decode_doc_id, encode_doc_id,
    encode_document, encode_drive, encode_entry, encode_revision, encode_revision_id,
    encode_snapshot, encode_tag, encode_token, encode_u32, encode_u64, encode_volume,
};
use hk_proto::status::{STATUS_ERROR, STATUS_OK};

/// 文件系统错误转协议状态码。
pub fn status(error: Error) -> u8 {
    error.status().as_u8()
}

/// 协议状态码还原文件系统错误；未知状态按 I/O 错误处理。
pub fn error(status: u8) -> Error {
    Error::from_status_code(status)
}

/// 把线编码结果包装成响应状态与长度。
pub fn encoded(length: Option<usize>) -> (u8, usize) {
    match length {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

/// 文档身份响应。
pub fn doc(body: &mut [u8], document: DocId) -> (u8, usize) {
    encoded(encode_doc_id(body, WireDocId::new(document.get())))
}

/// 文档信息响应。
pub fn info(body: &mut [u8], info: DocumentInfo) -> (u8, usize) {
    let view = DocumentView {
        kind: kind_code(info.kind),
        head: info.head.get(),
        parent: info.parent.map_or(0, |parent| parent.get()),
        size: info.size,
        moment: info.moment,
    };
    encoded(encode_document(body, view))
}

/// 集合项列表响应。
pub fn entries(body: &mut [u8], entries: &[EntryInfo]) -> (u8, usize) {
    let mut length = 0;
    for entry in entries {
        let Some(end) = encode_entry(
            body,
            length,
            kind_code(entry.kind),
            entry.document.get(),
            entry.name.as_str(),
        ) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

/// 修订列表响应。
pub fn revisions(body: &mut [u8], revisions: &[RevisionInfo]) -> (u8, usize) {
    let mut length = 0;
    for revision in revisions {
        let view = RevisionView {
            id: revision.id.get(),
            parent: revision.parent.map_or(0, |parent| parent.get()),
            size: revision.size,
            moment: revision.moment,
            flags: revision.flags.bits(),
        };
        let Some(end) = encode_revision(body, length, view) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

/// 修订身份响应。
pub fn revision(body: &mut [u8], revision: RevisionId) -> (u8, usize) {
    encoded(encode_revision_id(
        body,
        WireRevisionId::new(revision.get()),
    ))
}

/// 令牌响应。
pub fn token(body: &mut [u8], token: u64) -> (u8, usize) {
    encoded(encode_token(body, token))
}

/// 卷信息响应。
pub fn volume(body: &mut [u8], view: VolumeView) -> (u8, usize) {
    encoded(encode_volume(body, view))
}

/// 盘符列表响应。
pub fn drives<I: IntoIterator<Item = DriveView>>(body: &mut [u8], drives: I) -> (u8, usize) {
    let mut length = 0;
    for view in drives {
        let Some(end) = encode_drive(body, length, view) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

/// 快照列表响应。
pub fn snapshots(body: &mut [u8], snapshots: &[SnapshotInfo]) -> (u8, usize) {
    let mut length = 0;
    for snapshot in snapshots {
        let Some(end) = encode_snapshot(
            body,
            length,
            snapshot.id,
            snapshot.generation,
            snapshot.moment,
            snapshot.documents,
            snapshot.name.as_str(),
        ) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

/// 标签列表响应。
pub fn tags(body: &mut [u8], tags: &[TagInfo]) -> (u8, usize) {
    let mut length = 0;
    for tag in tags {
        let Some(end) = encode_tag(body, length, tag.id, tag.name.as_str()) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

/// 原始字节响应。
pub fn bytes(body: &mut [u8], content: &[u8]) -> (u8, usize) {
    if content.len() > body.len() {
        return (STATUS_ERROR, 0);
    }
    body[..content.len()].copy_from_slice(content);
    (STATUS_OK, content.len())
}

/// u64 响应。
pub fn number_u64(body: &mut [u8], value: u64) -> (u8, usize) {
    encoded(encode_u64(body, value))
}

/// u32 响应。
pub fn number_u32(body: &mut [u8], value: u32) -> (u8, usize) {
    encoded(encode_u32(body, value))
}

/// 文档身份列表响应。
pub fn list_docs(body: &mut [u8], documents: &[DocId]) -> (u8, usize) {
    let mut length: usize = 0;
    for document in documents {
        let Some(end) = length.checked_add(16) else {
            return (STATUS_ERROR, 0);
        };
        if end > body.len() {
            return (STATUS_ERROR, 0);
        }
        body[length..end].copy_from_slice(&document.to_le_bytes());
        length = end;
    }
    (STATUS_OK, length)
}

/// u32 列表响应。
pub fn list_u32(body: &mut [u8], values: &[u32]) -> (u8, usize) {
    let mut length: usize = 0;
    for value in values {
        let Some(end) = length.checked_add(4) else {
            return (STATUS_ERROR, 0);
        };
        if end > body.len() {
            return (STATUS_ERROR, 0);
        }
        body[length..end].copy_from_slice(&value.to_le_bytes());
        length = end;
    }
    (STATUS_OK, length)
}

/// 从请求体解析文档身份。
pub fn doc_from(bytes: &[u8]) -> Option<DocId> {
    decode_doc_id(bytes).map(|document| DocId::new(document.get()))
}

/// 文档类型编码。
pub const fn kind_code(kind: DocumentKind) -> u8 {
    match kind {
        DocumentKind::Content => KIND_CONTENT,
        DocumentKind::Collection => KIND_COLLECTION,
    }
}

/// 从请求参数解析文档类型。
pub const fn document_kind(code: u64) -> Option<DocumentKind> {
    match code {
        0 => Some(DocumentKind::Content),
        1 => Some(DocumentKind::Collection),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_proto::status::{
        STATUS_DAMAGED, STATUS_EXISTS, STATUS_INVALID, STATUS_IO, STATUS_NOT_FOUND,
        STATUS_PERMISSION, STATUS_READ_ONLY, STATUS_STALE,
    };

    #[test]
    fn maps_every_error_class() {
        assert_eq!(status(Error::NotFound), STATUS_NOT_FOUND);
        assert_eq!(status(Error::Exists), STATUS_EXISTS);
        assert_eq!(status(Error::Invalid), STATUS_INVALID);
        assert_eq!(status(Error::NotDirectory), STATUS_INVALID);
        assert_eq!(status(Error::IsDirectory), STATUS_INVALID);
        assert_eq!(status(Error::Io), STATUS_IO);
        assert_eq!(status(Error::Refused), STATUS_PERMISSION);
        assert_eq!(status(Error::Stale), STATUS_STALE);
        assert_eq!(status(Error::Damaged), STATUS_DAMAGED);
        assert_eq!(status(Error::ReadOnly), STATUS_READ_ONLY);
        assert_eq!(status(Error::Busy), STATUS_ERROR);
        assert_eq!(status(Error::Quota), STATUS_ERROR);
    }

    #[test]
    fn restores_errors_from_status() {
        assert_eq!(error(STATUS_NOT_FOUND), Error::NotFound);
        assert_eq!(error(STATUS_INVALID), Error::Invalid);
        assert_eq!(error(STATUS_IO), Error::Io);
        assert_eq!(error(STATUS_PERMISSION), Error::Refused);
        assert_eq!(error(STATUS_OK), Error::Io);
    }

    #[test]
    fn encodes_documents_and_lists() {
        let mut body = [0_u8; 64];
        let (status, length) = doc(&mut body, DocId::new(9));
        assert_eq!(status, STATUS_OK);
        assert_eq!(length, 16);
        assert_eq!(doc_from(&body[..length]), Some(DocId::new(9)));

        let (status, length) = list_docs(&mut body, &[DocId::new(1), DocId::new(2)]);
        assert_eq!(status, STATUS_OK);
        assert_eq!(length, 32);

        let (status, length) = list_u32(&mut body[..8], &[7, 8]);
        assert_eq!(status, STATUS_OK);
        assert_eq!(length, 8);
        assert_eq!(list_u32(&mut body[..4], &[7, 8]), (STATUS_ERROR, 0));

        let (status, length) = bytes(&mut body[..4], b"holy");
        assert_eq!(status, STATUS_OK);
        assert_eq!(length, 4);
        assert_eq!(bytes(&mut body[..2], b"long"), (STATUS_ERROR, 0));
    }

    #[test]
    fn maps_document_kinds() {
        assert_eq!(kind_code(DocumentKind::Content), KIND_CONTENT);
        assert_eq!(kind_code(DocumentKind::Collection), KIND_COLLECTION);
        assert_eq!(document_kind(0), Some(DocumentKind::Content));
        assert_eq!(document_kind(1), Some(DocumentKind::Collection));
        assert_eq!(document_kind(2), None);
    }
}
