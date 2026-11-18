//! Holy 文档请求分派。

use alloc::vec::Vec;

use hk_fs::{Backend, DocId, DocSessions, HandleRights, RevisionId};
use hk_proto::holy::{
    DriveView, OP_ACQUIRE, OP_CREATE, OP_DOCUMENT_ID, OP_DRIVES, OP_ENUMERATE_ID, OP_GRANT,
    OP_READ_ID, OP_READ_REVISION, OP_READ_TOKEN, OP_RELEASE, OP_RESOLVE, OP_REVISIONS_ID,
    OP_REVOKE, OP_ROOT, OP_SYNC, OP_TRANSFER, OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND,
    OP_VOLUME, OP_WRITE_ID, OP_WRITE_TOKEN, Request, TOKEN_RIGHT_READ, TOKEN_RIGHT_WRITE,
    VolumeView, decode_u64,
};
use hk_proto::vfs::{STATUS_INVALID, STATUS_NOT_FOUND, STATUS_OK, STATUS_PERMISSION};

use hk_holy_response::response;

use crate::Drives;

/// 通用文档协议覆盖的操作码。
///
/// 由 [`handle`] 分派；具备原生后端的服务不得再自行实现这些分支。
pub const SHARED_OPS: &[u8] = &[
    OP_DRIVES,
    OP_TRANSFER,
    OP_RELEASE,
    OP_REVOKE,
    OP_GRANT,
    OP_VOLUME,
    OP_ROOT,
    OP_RESOLVE,
    OP_CREATE,
    OP_UNBIND,
    OP_ENUMERATE_ID,
    OP_DOCUMENT_ID,
    OP_READ_ID,
    OP_READ_REVISION,
    OP_WRITE_ID,
    OP_TRUNCATE_ID,
    OP_REVISIONS_ID,
    OP_SYNC,
    OP_ACQUIRE,
    OP_READ_TOKEN,
    OP_WRITE_TOKEN,
    OP_TRUNCATE_TOKEN,
];

/// 盘符到文档后端的来源。
///
/// 通用分派只依赖该契约：进程内后端注册表与 HolyFS 自己的卷都能复用同一套
/// 协议处理，不需要各自维护分派表。
pub trait DriveSource {
    /// 列出全部盘符与卷视图。
    fn list(&self) -> Vec<(u8, VolumeView)>;

    /// 返回盘符的卷视图。
    fn volume(&self, letter: u8) -> Option<VolumeView>;

    /// 在盘符上执行一次后端操作；盘符不存在时返回 `None`。
    fn with_drive<R>(
        &mut self,
        letter: u8,
        f: impl FnOnce(&mut dyn Backend, DocId) -> R,
    ) -> Option<R>;
}

impl DriveSource for Drives {
    fn list(&self) -> Vec<(u8, VolumeView)> {
        Drives::list(self)
    }

    fn volume(&self, letter: u8) -> Option<VolumeView> {
        Drives::volume(self, letter)
    }

    fn with_drive<R>(
        &mut self,
        letter: u8,
        f: impl FnOnce(&mut dyn Backend, DocId) -> R,
    ) -> Option<R> {
        let drive = self.get_mut(letter)?;
        let root = drive.root;
        Some(f(drive.backend.as_mut(), root))
    }
}

/// 处理原生文档请求，返回状态与响应字节数。
pub fn handle<S: DriveSource>(
    drives: &mut S,
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
        return match sessions
            .doc(sender, letter)
            .grant(request.arg0, HandleRights::from_bits(request.arg1 as u32))
        {
            Ok(token) => response::token(body, token),
            Err(error) => (response::status(error), 0),
        };
    }
    if request.op == OP_VOLUME {
        let Some(volume) = drives.volume(letter) else {
            return (STATUS_NOT_FOUND, 0);
        };
        return response::volume(body, volume);
    }
    match request.op {
        OP_ROOT => with_drive(drives, letter, |_, root| response::doc(body, root)),
        OP_RESOLVE => with_drive(drives, letter, |backend, _| {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.resolve(collection, request.path) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_CREATE => with_drive(drives, letter, |backend, _| {
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
        }),
        OP_UNBIND => with_drive(drives, letter, |backend, _| {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.unbind(collection, request.path) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_ENUMERATE_ID => with_drive(drives, letter, |backend, _| {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.enumerate(document) {
                Ok(entries) => response::entries(body, &entries),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_DOCUMENT_ID => with_drive(drives, letter, |backend, _| {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.document(document) {
                Ok(info) => response::info(body, info),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_READ_ID => with_drive(drives, letter, |backend, _| {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let length = (request.arg1 as usize).min(body.len());
            match backend.read(document, None, request.arg0, &mut body[..length]) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_READ_REVISION => with_drive(drives, letter, |backend, _| {
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
        }),
        OP_WRITE_ID => with_drive(drives, letter, |backend, _| {
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
        }),
        OP_TRUNCATE_ID => with_drive(drives, letter, |backend, _| {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.truncate(document, request.arg0) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_REVISIONS_ID => with_drive(drives, letter, |backend, _| {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.revisions(document) {
                Ok(revisions) => response::revisions(body, &revisions),
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_SYNC => with_drive(drives, letter, |backend, _| match backend.sync() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        }),
        OP_ACQUIRE => with_drive(drives, letter, |backend, _| {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match backend.document(document) {
                Ok(info) => {
                    let token = sessions.doc(sender, letter).acquire(
                        document,
                        info.head,
                        HandleRights::from_bits(request.arg0 as u32),
                    );
                    response::token(body, token)
                }
                Err(error) => (response::status(error), 0),
            }
        }),
        OP_READ_TOKEN => with_drive(drives, letter, |backend, _| {
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
        }),
        OP_WRITE_TOKEN => with_drive(drives, letter, |backend, _| {
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
        }),
        OP_TRUNCATE_TOKEN => with_drive(drives, letter, |backend, _| {
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
        }),
        _ => (STATUS_INVALID, 0),
    }
}

fn with_drive<S: DriveSource>(
    drives: &mut S,
    letter: u8,
    f: impl FnOnce(&mut dyn Backend, DocId) -> (u8, usize),
) -> (u8, usize) {
    match drives.with_drive(letter, f) {
        Some(result) => result,
        None => (STATUS_NOT_FOUND, 0),
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

    fn fake_drives() -> Drives {
        let mut drives = Drives::new();
        assert!(drives.insert_local(
            b'H',
            Fake {
                root: DocId::new(7)
            }
        ));
        drives
    }

    #[test]
    fn dispatches_drives_roots_and_missing_drives() {
        let mut drives = fake_drives();
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
        let mut drives = fake_drives();
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

    #[test]
    fn shared_ops_are_implemented() {
        let mut drives = fake_drives();
        let mut sessions = DocSessions::new();
        let mut body = [0_u8; 64];
        let mut data = [0_u8; 24];
        data[..16].copy_from_slice(&7_u128.to_le_bytes());
        for &op in SHARED_OPS {
            let (status, _) = handle(
                &mut drives,
                &mut sessions,
                1,
                &request(op, b'H', &data),
                &mut body,
            );
            assert_ne!(status, STATUS_INVALID, "shared op {op:#x} 未实现");
        }
    }
}
