//! 原生 Holy 文档请求：DocPath、DocId、偏移读写与句柄令牌。

#![cfg(target_os = "none")]

use hk_block::ipc::ClientDisk;
use hk_fs_holyfs::{DocumentInfo, DocumentKind, HolyError, HolyFs, RevisionId};
use hk_server_api::holy::{
    DocId as WireDocId, DocumentView, KIND_COLLECTION, KIND_CONTENT, OP_ACQUIRE, OP_CREATE,
    OP_CREATE_TAG, OP_DOCUMENT, OP_DOCUMENT_ID, OP_DOCUMENT_TOKEN, OP_DROP_TAG, OP_ENUMERATE,
    OP_ENUMERATE_ID, OP_FORGET, OP_GRANT, OP_READ, OP_READ_ID, OP_READ_TOKEN, OP_RELEASE,
    OP_RESOLVE, OP_RESTORE, OP_REVISIONS, OP_REVISIONS_ID, OP_REVOKE, OP_ROOT, OP_SNAPSHOT,
    OP_SNAPSHOTS, OP_SYNC, OP_TAG, OP_TAG_MEMBERS, OP_TAGS, OP_TAGS_OF, OP_TRANSFER,
    OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND, OP_UNTAG, OP_WRITE_ID, OP_WRITE_TOKEN,
    RIGHT_INSPECT, RIGHT_READ, RIGHT_WRITE, RevisionId as WireRevisionId, RevisionView,
    SnapshotView, decode_doc_id, decode_u64, encode_doc_id, encode_document, encode_entry,
    encode_revision, encode_revision_id, encode_snapshot, encode_tag, encode_token, encode_u32,
    encode_u64,
};
use hk_server_api::vfs::{
    Request, STATUS_ERROR, STATUS_EXISTS, STATUS_INVALID, STATUS_IO, STATUS_NOT_FOUND, STATUS_OK,
    STATUS_PERMISSION,
};

use crate::sessions::Sessions;

/// 处理原生文档请求，返回状态与响应字节数。
pub(crate) fn handle(
    drive: Option<&mut HolyFs<ClientDisk>>,
    sessions: &mut Sessions,
    sender: u64,
    request: &Request<'_>,
    body: &mut [u8],
) -> (u8, usize) {
    let Some(drive) = drive else {
        return (STATUS_IO, 0);
    };
    if request.op == OP_RESTORE {
        return match drive.restore(request.arg0) {
            Ok(generation) => {
                sessions.invalidate_documents();
                encode_u64_response(body, generation)
            }
            Err(error) => (status(error), 0),
        };
    }
    if request.op == OP_TRANSFER {
        return match sessions.transfer(sender, request.arg1, request.arg0) {
            Ok(token) => encode_token_response(body, token),
            Err(status) => (status, 0),
        };
    }
    let session = sessions.doc(sender);
    match request.op {
        OP_ENUMERATE => match drive.enumerate(request.path) {
            Ok(entries) => encode_entries(body, entries),
            Err(error) => (status(error), 0),
        },
        OP_READ => match drive.read_document(request.path) {
            Ok(content) => copy(body, &content),
            Err(error) => (status(error), 0),
        },
        OP_DOCUMENT => match drive.document(request.path) {
            Ok(info) => encode_info(body, info),
            Err(error) => (status(error), 0),
        },
        OP_REVISIONS => match drive.revisions(request.path) {
            Ok(revisions) => encode_revisions(body, revisions),
            Err(error) => (status(error), 0),
        },
        OP_ROOT => encode_doc(body, drive.root()),
        OP_RESOLVE => {
            let Some(collection) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.lookup(collection, request.path) {
                Ok(document) => encode_doc(body, document),
                Err(error) => (status(error), 0),
            }
        }
        OP_CREATE => {
            let Some(collection) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let Some(kind) = document_kind(request.arg0) else {
                return (STATUS_INVALID, 0);
            };
            match drive.create(collection, request.path, kind) {
                Ok(document) => encode_doc(body, document),
                Err(error) => (status(error), 0),
            }
        }
        OP_UNBIND => {
            let Some(collection) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.unlink(collection, request.path) {
                Ok(document) => encode_doc(body, document),
                Err(error) => (status(error), 0),
            }
        }
        OP_ENUMERATE_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.enumerate_document(document) {
                Ok(entries) => encode_entries(body, entries),
                Err(error) => (status(error), 0),
            }
        }
        OP_DOCUMENT_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.document_info(document) {
                Ok(info) => encode_info(body, info),
                Err(error) => (status(error), 0),
            }
        }
        OP_READ_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let length = (request.arg1 as usize).min(body.len());
            match drive.read_range(document, request.arg0, &mut body[..length]) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let content = &request.data[16..];
            match drive.write_range(document, request.arg0, content) {
                Ok(_) => match drive.document_info(document) {
                    Ok(info) => encode_revision_response(body, info.head),
                    Err(error) => (status(error), 0),
                },
                Err(error) => (status(error), 0),
            }
        }
        OP_TRUNCATE_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.truncate(document, request.arg0) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_REVISIONS_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.revisions_of(document) {
                Ok(revisions) => encode_revisions(body, revisions),
                Err(error) => (status(error), 0),
            }
        }
        OP_SYNC => match drive.sync() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_ACQUIRE => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.document_info(document) {
                Ok(info) => {
                    let token = session.acquire(document, info.head, request.arg0 as u32);
                    encode_token_response(body, token)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_RELEASE | OP_REVOKE => {
            if session.release(request.arg0) {
                (STATUS_OK, 0)
            } else {
                (STATUS_NOT_FOUND, 0)
            }
        }
        OP_GRANT => match session.grant(request.arg0, request.arg1 as u32) {
            Ok(token) => encode_token_response(body, token),
            Err(error) => (status(error), 0),
        },
        OP_READ_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(status) => return (status, 0),
            };
            if token.rights.bits() & RIGHT_READ == 0 {
                return (STATUS_PERMISSION, 0);
            }
            let Some(length) = decode_u64(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let length = (length as usize).min(body.len());
            match drive.read_range_at(
                token.document,
                token.revision,
                request.arg1,
                &mut body[..length],
            ) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(status) => return (status, 0),
            };
            if token.rights.bits() & RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match drive.write_range(token.document, request.arg1, request.data) {
                Ok(_) => match drive.document_info(token.document) {
                    Ok(info) => {
                        session.set_revision(request.arg0, info.head);
                        encode_revision_response(body, info.head)
                    }
                    Err(error) => (status(error), 0),
                },
                Err(error) => (status(error), 0),
            }
        }
        OP_TRUNCATE_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(status) => return (status, 0),
            };
            if token.rights.bits() & RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match drive.truncate(token.document, request.arg1) {
                Ok(()) => match drive.document_info(token.document) {
                    Ok(info) => {
                        session.set_revision(request.arg0, info.head);
                        (STATUS_OK, 0)
                    }
                    Err(error) => (status(error), 0),
                },
                Err(error) => (status(error), 0),
            }
        }
        OP_DOCUMENT_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(status) => return (status, 0),
            };
            if token.rights.bits() & RIGHT_INSPECT == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match drive.document_info(token.document) {
                Ok(info) => encode_info(body, info),
                Err(error) => (status(error), 0),
            }
        }
        OP_SNAPSHOT => match drive.snapshot(request.path) {
            Ok(snapshot) => encode_u64_response(body, snapshot),
            Err(error) => (status(error), 0),
        },
        OP_SNAPSHOTS => encode_snapshots(body, drive.snapshots()),
        OP_FORGET => match drive.forget(request.arg0) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_TAGS => encode_tags(body, drive.tags()),
        OP_TAG_MEMBERS => match drive.tag_members(request.arg0 as u32) {
            Ok(documents) => encode_doc_id_list(body, &documents),
            Err(error) => (status(error), 0),
        },
        OP_CREATE_TAG => match drive.create_tag(request.path) {
            Ok(tag) => encode_u32_response(body, tag),
            Err(error) => (status(error), 0),
        },
        OP_DROP_TAG => match drive.drop_tag(request.arg0 as u32) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_TAG => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.tag(document, request.arg0 as u32) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_UNTAG => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.untag(document, request.arg0 as u32) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_TAGS_OF => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.tags_of(document) {
                Ok(tags) => encode_u32_list(body, &tags),
                Err(error) => (status(error), 0),
            }
        }
        _ => (STATUS_INVALID, 0),
    }
}

fn encode_entries(
    body: &mut [u8],
    entries: alloc::vec::Vec<hk_fs_holyfs::EntryInfo>,
) -> (u8, usize) {
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

fn encode_revisions(
    body: &mut [u8],
    revisions: alloc::vec::Vec<hk_fs_holyfs::RevisionInfo>,
) -> (u8, usize) {
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

fn encode_info(body: &mut [u8], info: DocumentInfo) -> (u8, usize) {
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

fn encode_doc(body: &mut [u8], document: hk_fs_holyfs::DocId) -> (u8, usize) {
    match encode_doc_id(body, WireDocId::new(document.get())) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_revision_response(body: &mut [u8], revision: RevisionId) -> (u8, usize) {
    match encode_revision_id(body, WireRevisionId::new(revision.get())) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn doc_from(bytes: &[u8]) -> Option<hk_fs_holyfs::DocId> {
    decode_doc_id(bytes).map(|document| hk_fs_holyfs::DocId::new(document.get()))
}

fn encode_token_response(body: &mut [u8], token: u64) -> (u8, usize) {
    match encode_token(body, token) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_snapshots(
    body: &mut [u8],
    snapshots: alloc::vec::Vec<hk_fs_holyfs::SnapshotInfo>,
) -> (u8, usize) {
    let mut length = 0;
    for snapshot in snapshots {
        let view = SnapshotView {
            id: snapshot.id,
            generation: snapshot.generation,
            moment: snapshot.moment,
            documents: snapshot.documents,
            name: &snapshot.name,
        };
        let Some(end) = encode_snapshot(
            body,
            length,
            view.id,
            view.generation,
            view.moment,
            view.documents,
            view.name,
        ) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

fn encode_u64_response(body: &mut [u8], value: u64) -> (u8, usize) {
    match encode_u64(body, value) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_tags(body: &mut [u8], tags: alloc::vec::Vec<hk_fs::TagInfo>) -> (u8, usize) {
    let mut length = 0;
    for tag in tags {
        let Some(end) = encode_tag(body, length, tag.id, &tag.name) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

fn encode_doc_id_list(body: &mut [u8], documents: &[hk_fs_holyfs::DocId]) -> (u8, usize) {
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

fn encode_u32_response(body: &mut [u8], value: u32) -> (u8, usize) {
    match encode_u32(body, value) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_u32_list(body: &mut [u8], values: &[u32]) -> (u8, usize) {
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

fn copy(body: &mut [u8], content: &[u8]) -> (u8, usize) {
    if content.len() > body.len() {
        return (STATUS_ERROR, 0);
    }
    body[..content.len()].copy_from_slice(content);
    (STATUS_OK, content.len())
}

fn kind_of(kind: DocumentKind) -> u8 {
    match kind {
        DocumentKind::Content => KIND_CONTENT,
        DocumentKind::Collection => KIND_COLLECTION,
    }
}

fn document_kind(code: u64) -> Option<DocumentKind> {
    match code {
        0 => Some(DocumentKind::Content),
        1 => Some(DocumentKind::Collection),
        _ => None,
    }
}

fn status(error: HolyError) -> u8 {
    match error {
        HolyError::NotFound => STATUS_NOT_FOUND,
        HolyError::Exists => STATUS_EXISTS,
        HolyError::Invalid | HolyError::NotDirectory | HolyError::IsDirectory => STATUS_INVALID,
        HolyError::Io => STATUS_IO,
        HolyError::Refused => STATUS_PERMISSION,
        _ => STATUS_ERROR,
    }
}
