//! Holy 文档请求分派。

use hk_fs::{DocSessions, RevisionId};
use hk_proto::holy::{
    DriveView, OP_ACQUIRE, OP_CREATE, OP_DOCUMENT_ID, OP_DRIVES, OP_ENUMERATE_ID, OP_GRANT,
    OP_READ_ID, OP_READ_REVISION, OP_READ_TOKEN, OP_RELEASE, OP_RESOLVE, OP_REVISIONS_ID,
    OP_REVOKE, OP_ROOT, OP_SYNC, OP_TRANSFER, OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND,
    OP_VOLUME, OP_WRITE_ID, OP_WRITE_TOKEN, Request, TOKEN_RIGHT_READ, TOKEN_RIGHT_WRITE,
    decode_u64,
};
use hk_proto::vfs::{STATUS_INVALID, STATUS_NOT_FOUND, STATUS_OK, STATUS_PERMISSION};

use hk_holy_proto::response;

use crate::Drives;

/// 处理原生 VFS 请求。
pub fn handle(
    drives: &mut Drives,
    sessions: &mut DocSessions,
    sender: u64,
    request: &Request<'_>,
    body: &mut [u8],
) -> (u8, usize) {
    if request.op == OP_DRIVES {
        return response::drives(
            body,
            drives
                .list()
                .into_iter()
                .map(|(letter, volume)| DriveView { letter, volume }),
        );
    }
    let letter = if request.drive == 0 {
        b'H'
    } else {
        request.drive
    };
    if request.op == OP_TRANSFER {
        return match sessions.transfer(sender, request.arg1, request.arg0, letter) {
            Ok(token) => response::token(body, token),
            Err(error) => (response::status(error), 0),
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
        return match sessions.doc(sender, letter).grant(
            request.arg0,
            hk_fs::HandleRights::from_bits(request.arg1 as u32),
        ) {
            Ok(token) => response::token(body, token),
            Err(error) => (response::status(error), 0),
        };
    }
    let Some(entry) = drives.get_mut(letter) else {
        return (STATUS_NOT_FOUND, 0);
    };
    let volume = entry.volume;
    let root = entry.root;
    let backend = entry.backend.as_mut();
    if request.op == OP_VOLUME {
        return response::volume(body, volume);
    }
    match request.op {
        OP_ROOT => response::doc(body, root),
        OP_RESOLVE => {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.resolve(collection, request.path) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_CREATE => {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let Some(kind) = response::document_kind(request.arg0) else {
                return (STATUS_INVALID, 0);
            };
            match backend.create(collection, request.path, kind) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_UNBIND => {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.unbind(collection, request.path) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_ENUMERATE_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.enumerate(document) {
                Ok(entries) => response::entries(body, &entries),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_DOCUMENT_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.document(document) {
                Ok(info) => response::info(body, info),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_READ_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let length = (request.arg1 as usize).min(body.len());
            match backend.read(document, None, request.arg0, &mut body[..length]) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_READ_REVISION => {
            let Some(document) = response::doc_from(request.data) else {
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
                Err(error) => (response::status(error), 0),
            }
        }
        OP_WRITE_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let content = &request.data[16..];
            match backend.write(document, request.arg0, content) {
                Ok(_) => match backend.document(document) {
                    Ok(info) => response::revision(body, info.head),
                    Err(error) => (response::status(error), 0),
                },
                Err(error) => (response::status(error), 0),
            }
        }
        OP_TRUNCATE_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.truncate(document, request.arg0) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_REVISIONS_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.revisions(document) {
                Ok(revisions) => response::revisions(body, &revisions),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_SYNC => match backend.sync() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        },
        OP_ACQUIRE => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.document(document) {
                Ok(info) => {
                    let token = sessions.doc(sender, letter).acquire(
                        document,
                        info.head,
                        hk_fs::HandleRights::from_bits(request.arg0 as u32),
                    );
                    response::token(body, token)
                }
                Err(error) => (response::status(error), 0),
            }
        }
        OP_READ_TOKEN => {
            let token = match sessions.doc(sender, letter).get(request.arg0) {
                Ok(token) => token,
                Err(error) => return (response::status(error), 0),
            };
            if token.rights.bits() & TOKEN_RIGHT_READ == 0 {
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
                Err(error) => (response::status(error), 0),
            }
        }
        OP_WRITE_TOKEN => {
            let token = match sessions.doc(sender, letter).get(request.arg0) {
                Ok(token) => token,
                Err(error) => return (response::status(error), 0),
            };
            if token.rights.bits() & TOKEN_RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match backend.write(token.document, request.arg1, request.data) {
                Ok(_) => match backend.document(token.document) {
                    Ok(info) => {
                        sessions
                            .doc(sender, letter)
                            .set_revision(request.arg0, info.head);
                        response::revision(body, info.head)
                    }
                    Err(error) => (response::status(error), 0),
                },
                Err(error) => (response::status(error), 0),
            }
        }
        OP_TRUNCATE_TOKEN => {
            let token = match sessions.doc(sender, letter).get(request.arg0) {
                Ok(token) => token,
                Err(error) => return (response::status(error), 0),
            };
            if token.rights.bits() & TOKEN_RIGHT_WRITE == 0 {
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
                    Err(error) => (response::status(error), 0),
                },
                Err(error) => (response::status(error), 0),
            }
        }
        _ => (STATUS_INVALID, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    use hk_fs::{
        Backend, Caps, DocId, DocumentInfo, DocumentKind, EntryInfo, Error, RevisionId,
        RevisionInfo,
    };
    use hk_proto::holy::{OP_DOCUMENT_ID, OP_DRIVES, OP_ROOT, decode_drives};
    use hk_proto::vfs::{STATUS_INVALID, STATUS_NOT_FOUND, STATUS_OK};

    struct Fake {
        root: DocId,
    }

    impl Backend for Fake {
        fn root(&self) -> DocId {
            self.root
        }

        fn caps(&self) -> Caps {
            Caps::NATIVE
        }

        fn document(&self, document: DocId) -> Result<DocumentInfo, Error> {
            Ok(DocumentInfo {
                id: document,
                kind: DocumentKind::Collection,
                head: RevisionId::ROOT,
                parent: None,
                size: 0,
                moment: 0,
            })
        }

        fn resolve(&self, _collection: DocId, _name: &str) -> Result<DocId, Error> {
            Err(Error::NotFound)
        }

        fn enumerate(&self, _collection: DocId) -> Result<Vec<EntryInfo>, Error> {
            Ok(Vec::new())
        }

        fn create(
            &mut self,
            _collection: DocId,
            _name: &str,
            _kind: DocumentKind,
        ) -> Result<DocId, Error> {
            Err(Error::ReadOnly)
        }

        fn unbind(&mut self, _collection: DocId, _name: &str) -> Result<DocId, Error> {
            Err(Error::ReadOnly)
        }

        fn read(
            &self,
            _document: DocId,
            _revision: Option<RevisionId>,
            _offset: u64,
            _buffer: &mut [u8],
        ) -> Result<usize, Error> {
            Ok(0)
        }

        fn write(&mut self, _document: DocId, _offset: u64, _data: &[u8]) -> Result<usize, Error> {
            Err(Error::ReadOnly)
        }

        fn truncate(&mut self, _document: DocId, _size: u64) -> Result<(), Error> {
            Err(Error::ReadOnly)
        }

        fn revisions(&self, _document: DocId) -> Result<Vec<RevisionInfo>, Error> {
            Ok(Vec::new())
        }
    }

    fn request<'a>(op: u8, drive: u8, data: &'a [u8]) -> Request<'a> {
        Request {
            op,
            drive,
            path: "",
            arg0: 0,
            arg1: 0,
            data,
        }
    }

    #[test]
    fn dispatches_drives_roots_and_missing_drives() {
        let mut drives = Drives::new();
        assert!(drives.insert_local(
            b'H',
            Fake {
                root: DocId::new(7)
            }
        ));
        let mut sessions = DocSessions::new();
        let mut body = [0_u8; 64];

        let (status, written) = handle(
            &mut drives,
            &mut sessions,
            1,
            &request(OP_DRIVES, 0, &[]),
            &mut body,
        );
        assert_eq!(status, STATUS_OK);
        let listed: Vec<_> = decode_drives(&body[..written]).collect();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].letter, b'H');
        assert_eq!(listed[0].volume.root.get(), 7);

        let (status, written) = handle(
            &mut drives,
            &mut sessions,
            1,
            &request(OP_ROOT, b'H', &[]),
            &mut body,
        );
        assert_eq!(status, STATUS_OK);
        assert_eq!(&body[..written], &7_u128.to_le_bytes());

        let (status, _) = handle(
            &mut drives,
            &mut sessions,
            1,
            &request(OP_ROOT, b'Z', &[]),
            &mut body,
        );
        assert_eq!(status, STATUS_NOT_FOUND);
    }

    #[test]
    fn rejects_malformed_document_payloads() {
        let mut drives = Drives::new();
        assert!(drives.insert_local(
            b'H',
            Fake {
                root: DocId::new(7)
            }
        ));
        let mut sessions = DocSessions::new();
        let mut body = [0_u8; 64];

        for length in 0..16 {
            let data = [0xFF_u8; 16];
            let (status, _) = handle(
                &mut drives,
                &mut sessions,
                1,
                &request(OP_DOCUMENT_ID, b'H', &data[..length]),
                &mut body,
            );
            assert_eq!(status, STATUS_INVALID);
        }
    }
}
