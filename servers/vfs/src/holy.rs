//! 原生 Holy 文档请求：按 DocPath 枚举、整文档读取与修订视图。

#![cfg(target_os = "none")]

use hk_block::ipc::ClientDisk;
use hk_fs_holyfs::{DocumentKind, HolyError, HolyFs};
use hk_server_api::holy::{
    DocumentView, KIND_COLLECTION, KIND_CONTENT, OP_DOCUMENT, OP_ENUMERATE, OP_READ, OP_REVISIONS,
    RevisionView, encode_document, encode_entry, encode_revision,
};
use hk_server_api::vfs::{
    Request, STATUS_ERROR, STATUS_INVALID, STATUS_IO, STATUS_NOT_FOUND, STATUS_OK,
};

/// 处理原生文档请求，返回状态与响应字节数。
pub(crate) fn handle(
    drive: Option<&HolyFs<ClientDisk>>,
    request: &Request<'_>,
    body: &mut [u8],
) -> (u8, usize) {
    let Some(drive) = drive else {
        return (STATUS_IO, 0);
    };
    match request.op {
        OP_ENUMERATE => match drive.enumerate(request.path) {
            Ok(entries) => {
                let mut length = 0;
                for entry in entries {
                    let Some(end) = encode_entry(
                        body,
                        length,
                        kind_of(entry.kind),
                        entry.document.get(),
                        &entry.name,
                    ) else {
                        return (STATUS_ERROR, 0);
                    };
                    length = end;
                }
                (STATUS_OK, length)
            }
            Err(error) => (status(error), 0),
        },
        OP_READ => match drive.read_document(request.path) {
            Ok(content) => {
                if content.len() > body.len() {
                    return (STATUS_ERROR, 0);
                }
                body[..content.len()].copy_from_slice(&content);
                (STATUS_OK, content.len())
            }
            Err(error) => (status(error), 0),
        },
        OP_DOCUMENT => match drive.document(request.path) {
            Ok(info) => {
                let view = DocumentView {
                    kind: kind_of(info.kind),
                    head: info.head.get(),
                    parent: info.parent.map_or(0, |parent| parent.get()),
                    size: info.size,
                    moment: info.moment,
                };
                match encode_document(body, view) {
                    Some(length) => (STATUS_OK, length),
                    None => (STATUS_ERROR, 0),
                }
            }
            Err(error) => (status(error), 0),
        },
        OP_REVISIONS => match drive.revisions(request.path) {
            Ok(revisions) => {
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
            Err(error) => (status(error), 0),
        },
        _ => (STATUS_INVALID, 0),
    }
}

fn kind_of(kind: DocumentKind) -> u8 {
    match kind {
        DocumentKind::Content => KIND_CONTENT,
        DocumentKind::Collection => KIND_COLLECTION,
    }
}

fn status(error: HolyError) -> u8 {
    match error {
        HolyError::NotFound => STATUS_NOT_FOUND,
        HolyError::Invalid | HolyError::NotDirectory | HolyError::IsDirectory => STATUS_INVALID,
        HolyError::Io => STATUS_IO,
        _ => STATUS_ERROR,
    }
}
