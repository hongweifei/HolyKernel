//! 原生 VFS 的 Holy 文档请求处理。

#![cfg(target_os = "none")]

use hk_fs::{DocId, DocumentKind, Error as FsError, RevisionId};
use hk_server_api::holy::{
    DocId as WireDocId, DocumentView, KIND_COLLECTION, KIND_CONTENT, OP_ACQUIRE, OP_CREATE,
    OP_DOCUMENT_ID, OP_DRIVES, OP_ENUMERATE_ID, OP_GRANT, OP_READ_ID, OP_READ_REVISION,
    OP_READ_TOKEN, OP_RELEASE, OP_RESOLVE, OP_REVISIONS_ID, OP_REVOKE, OP_ROOT, OP_SYNC,
    OP_TRANSFER, OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND, OP_VOLUME, OP_WRITE_ID,
    OP_WRITE_TOKEN, RIGHT_READ, RIGHT_WRITE, Request, RevisionId as WireRevisionId, RevisionView,
    VolumeView, decode_doc_id, decode_u64, encode_doc_id, encode_document, encode_drive,
    encode_entry, encode_revision, encode_revision_id, encode_token,
};
use hk_server_api::vfs::{
    STATUS_DAMAGED, STATUS_ERROR, STATUS_EXISTS, STATUS_INVALID, STATUS_IO, STATUS_NOT_FOUND,
    STATUS_OK, STATUS_PERMISSION, STATUS_READ_ONLY, STATUS_STALE,
};

use crate::drives::Drives;
use crate::sessions::Sessions;

/// 处理原生 VFS 请求。
pub(crate) fn handle(
    drives: &mut Drives,
    sessions: &mut Sessions,
    sender: u64,
    request: &Request<'_>,
    body: &mut [u8],
) -> (u8, usize) {
    if request.op == OP_DRIVES {
        return encode_drives(body, drives.list());
    }
    let letter = if request.drive == 0 {
        b'H'
    } else {
        request.drive
    };
    if request.op == OP_TRANSFER {
        return match sessions.transfer(sender, request.arg1, request.arg0, letter) {
            Ok(token) => encode_token_response(body, token),
            Err(status) => (status, 0),
        };
    }
    if request.op == OP_RELEASE || request.op == OP_REVOKE {
        return if sessions.doc(sender, letter).release(request.arg0) {
            (STATUS_OK, 0)
        } else {
            (STATUS_NOT_FOUND, 0)
        };
    }
    if request.op == OP_GRANT {
        return match sessions
            .doc(sender, letter)
            .grant(request.arg0, request.arg1 as u32)
        {
            Ok(token) => encode_token_response(body, token),
            Err(error) => (status(error), 0),
        };
    }
    let Some(entry) = drives.get_mut(letter) else {
        return (STATUS_NOT_FOUND, 0);
    };
    let volume = entry.volume;
    let root = entry.root;
    let backend = entry.backend.as_mut();
    if request.op == OP_VOLUME {
        return encode_volume_response(body, volume);
    }
    match request.op {
        OP_ROOT => encode_doc(body, root),
        OP_RESOLVE => {
            let Some(collection) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.resolve(collection, request.path) {
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
            match backend.create(collection, request.path, kind) {
                Ok(document) => encode_doc(body, document),
                Err(error) => (status(error), 0),
            }
        }
        OP_UNBIND => {
            let Some(collection) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.unbind(collection, request.path) {
                Ok(document) => encode_doc(body, document),
                Err(error) => (status(error), 0),
            }
        }
        OP_ENUMERATE_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.enumerate(document) {
                Ok(entries) => encode_entries(body, entries),
                Err(error) => (status(error), 0),
            }
        }
        OP_DOCUMENT_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.document(document) {
                Ok(info) => encode_info(body, info),
                Err(error) => (status(error), 0),
            }
        }
        OP_READ_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let length = (request.arg1 as usize).min(body.len());
            match backend.read(document, None, request.arg0, &mut body[..length]) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (status(error), 0),
            }
        }
        OP_READ_REVISION => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let Some(revision) = request
                .data
                .get(16..24)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u64::from_le_bytes)
            else {
                return (STATUS_INVALID, 0);
            };
            let length = (request.arg1 as usize).min(body.len());
            match backend.read(
                document,
                Some(RevisionId::new(revision)),
                request.arg0,
                &mut body[..length],
            ) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let content = &request.data[16..];
            match backend.write(document, request.arg0, content) {
                Ok(_) => match backend.document(document) {
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
            match backend.truncate(document, request.arg0) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_REVISIONS_ID => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.revisions(document) {
                Ok(revisions) => encode_revisions(body, revisions),
                Err(error) => (status(error), 0),
            }
        }
        OP_SYNC => match backend.sync() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_ACQUIRE => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.document(document) {
                Ok(info) => {
                    let token = sessions.doc(sender, letter).acquire(
                        document,
                        info.head,
                        request.arg0 as u32,
                    );
                    encode_token_response(body, token)
                }
                Err(error) => (status(error), 0),
            }
        }
        OP_READ_TOKEN => {
            let token = match sessions.doc(sender, letter).get(request.arg0) {
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
            match backend.read(
                token.document,
                Some(token.revision),
                request.arg1,
                &mut body[..length],
            ) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_TOKEN => {
            let token = match sessions.doc(sender, letter).get(request.arg0) {
                Ok(token) => token,
                Err(status) => return (status, 0),
            };
            if token.rights.bits() & RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match backend.write(token.document, request.arg1, request.data) {
                Ok(_) => match backend.document(token.document) {
                    Ok(info) => {
                        sessions
                            .doc(sender, letter)
                            .set_revision(request.arg0, info.head);
                        encode_revision_response(body, info.head)
                    }
                    Err(error) => (status(error), 0),
                },
                Err(error) => (status(error), 0),
            }
        }
        OP_TRUNCATE_TOKEN => {
            let token = match sessions.doc(sender, letter).get(request.arg0) {
                Ok(token) => token,
                Err(status) => return (status, 0),
            };
            if token.rights.bits() & RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match backend.truncate(token.document, request.arg1) {
                Ok(()) => match backend.document(token.document) {
                    Ok(info) => {
                        sessions
                            .doc(sender, letter)
                            .set_revision(request.arg0, info.head);
                        (STATUS_OK, 0)
                    }
                    Err(error) => (status(error), 0),
                },
                Err(error) => (status(error), 0),
            }
        }
        _ => (STATUS_INVALID, 0),
    }
}

fn encode_entries(body: &mut [u8], entries: alloc::vec::Vec<hk_fs::EntryInfo>) -> (u8, usize) {
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
    revisions: alloc::vec::Vec<hk_fs::RevisionInfo>,
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

fn encode_info(body: &mut [u8], info: hk_fs::DocumentInfo) -> (u8, usize) {
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

fn encode_doc(body: &mut [u8], document: DocId) -> (u8, usize) {
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

fn encode_token_response(body: &mut [u8], token: u64) -> (u8, usize) {
    match encode_token(body, token) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_volume_response(body: &mut [u8], view: VolumeView) -> (u8, usize) {
    match hk_server_api::holy::encode_volume(body, view) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_drives(body: &mut [u8], drives: alloc::vec::Vec<(u8, VolumeView)>) -> (u8, usize) {
    let mut length = 0;
    for (letter, volume) in drives {
        let Some(end) = encode_drive(
            body,
            length,
            hk_server_api::holy::DriveView { letter, volume },
        ) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

fn doc_from(bytes: &[u8]) -> Option<DocId> {
    decode_doc_id(bytes).map(|document| DocId::new(document.get()))
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

fn status(error: FsError) -> u8 {
    match error {
        FsError::NotFound => STATUS_NOT_FOUND,
        FsError::Exists => STATUS_EXISTS,
        FsError::Invalid | FsError::NotDirectory | FsError::IsDirectory => STATUS_INVALID,
        FsError::Io => STATUS_IO,
        FsError::Refused => STATUS_PERMISSION,
        FsError::Stale => STATUS_STALE,
        FsError::Damaged => STATUS_DAMAGED,
        FsError::ReadOnly => STATUS_READ_ONLY,
        _ => STATUS_ERROR,
    }
}
