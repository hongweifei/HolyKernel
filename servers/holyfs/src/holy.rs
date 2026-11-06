//! 原生 Holy 文档请求：DocPath、DocId、偏移读写与句柄令牌。

#![cfg(target_os = "none")]

use hk_fs_holyfs::{DocumentInfo, DocumentKind, HolyError, Quota, RevisionId};
use hk_proto::holy::{
    DocId as WireDocId, DocumentView, DriveView, KIND_COLLECTION, KIND_CONTENT, MaintenanceView,
    OP_ABORT_STAGING, OP_ACQUIRE, OP_ACQUIRE_VOLUME, OP_ATTACH_DRIVE, OP_CHECK, OP_COMMIT_STAGING,
    OP_CREATE, OP_CREATE_TAG, OP_DETACH_DRIVE, OP_DOCUMENT, OP_DOCUMENT_ID, OP_DOCUMENT_TOKEN,
    OP_DRIVES, OP_DROP_TAG, OP_ENUMERATE, OP_ENUMERATE_ID, OP_FORGET, OP_GRANT, OP_GRANT_VOLUME,
    OP_PRUNE, OP_PURGE, OP_QUOTA, OP_READ, OP_READ_ID, OP_READ_REVISION, OP_READ_TOKEN, OP_RELEASE,
    OP_RELEASE_VOLUME, OP_REPAIR, OP_RESOLVE, OP_RESTORE, OP_REVISIONS, OP_REVISIONS_ID, OP_REVOKE,
    OP_REVOKE_VOLUME, OP_ROOT, OP_SCRUB, OP_SEAL, OP_SET_QUOTA, OP_SNAPSHOT, OP_SNAPSHOTS,
    OP_STAGE, OP_SYNC, OP_TAG, OP_TAG_MEMBERS, OP_TAGS, OP_TAGS_OF, OP_TRANSFER, OP_TRUNCATE_ID,
    OP_TRUNCATE_TOKEN, OP_UNBIND, OP_UNTAG, OP_VOLUME, OP_WRITE_ID, OP_WRITE_STAGING,
    OP_WRITE_TOKEN, RIGHT_ADMIN, RIGHT_INSPECT, RIGHT_READ, RIGHT_WRITE, Request,
    RevisionId as WireRevisionId, RevisionView, SnapshotView, VolumeView, decode_doc_id,
    decode_quota, decode_u64, encode_doc_id, encode_document, encode_drive, encode_entry,
    encode_maintenance, encode_quota, encode_revision, encode_revision_id, encode_snapshot,
    encode_tag, encode_token, encode_u32, encode_u64, encode_volume,
};
use hk_proto::vfs::{
    STATUS_DAMAGED, STATUS_ERROR, STATUS_EXISTS, STATUS_INVALID, STATUS_IO, STATUS_NOT_FOUND,
    STATUS_OK, STATUS_PERMISSION, STATUS_READ_ONLY, STATUS_STALE,
};

use crate::read_cache::ReadCache;
use crate::sessions::Sessions;
use crate::volumes::{VolumeInfo, Volumes, parse_drive};

/// 对一个盘符执行一次提交；供 group commit 统一刷写。
pub(crate) fn sync(volumes: &mut Volumes, letter: u8) -> u8 {
    let Some(drive) = volumes.get_mut(letter) else {
        return STATUS_NOT_FOUND;
    };
    match drive.sync() {
        Ok(()) => STATUS_OK,
        Err(error) => status(error),
    }
}

/// 处理原生文档请求，返回状态与响应字节数。
pub(crate) fn handle(
    volumes: &mut Volumes,
    sessions: &mut Sessions,
    cache: &mut ReadCache,
    sender: u64,
    endpoint: u64,
    request: &Request<'_>,
    body: &mut [u8],
) -> (u8, usize) {
    if request.op == OP_DRIVES {
        return encode_drives(body, volumes.list());
    }
    if request.op == OP_ATTACH_DRIVE || request.op == OP_DETACH_DRIVE {
        let auth = if request.drive == 0 {
            b'H'
        } else {
            request.drive
        };
        let Some(volume) = volumes.get_mut(auth) else {
            return (STATUS_NOT_FOUND, 0);
        };
        let (volume_id, epoch) = (volume.volume_id(), volume.epoch());
        if let Err(status) =
            sessions
                .volume(sender, auth)
                .check(request.arg0, volume_id, epoch, RIGHT_ADMIN)
        {
            return (status, 0);
        }
        let Some(target) = parse_drive(request.path) else {
            return (STATUS_INVALID, 0);
        };
        let result = if request.op == OP_ATTACH_DRIVE {
            let Ok(service) = core::str::from_utf8(request.data) else {
                return (STATUS_INVALID, 0);
            };
            volumes.attach(target, service, endpoint)
        } else {
            volumes.detach(target)
        };
        return match result {
            Ok(()) => (STATUS_OK, 0),
            Err(status) => (status, 0),
        };
    }
    let letter = if request.drive == 0 {
        b'H'
    } else {
        request.drive
    };
    let Some(drive) = volumes.get_mut(letter) else {
        return (STATUS_NOT_FOUND, 0);
    };
    if request.op == OP_VOLUME {
        let view = VolumeView {
            volume: drive.volume_id().get(),
            generation: drive.generation(),
            epoch: drive.epoch(),
            root: WireDocId::new(drive.root().get()),
        };
        return encode_volume_response(body, view);
    }
    if request.op == OP_ACQUIRE_VOLUME {
        let token = sessions.volume(sender, letter).acquire(
            drive.volume_id(),
            drive.epoch(),
            request.arg0 as u32,
        );
        return encode_token_response(body, token);
    }
    if request.op == OP_RELEASE_VOLUME || request.op == OP_REVOKE_VOLUME {
        return match sessions.volume(sender, letter).release(
            request.arg0,
            drive.volume_id(),
            drive.epoch(),
        ) {
            Ok(()) => (STATUS_OK, 0),
            Err(status) => (status, 0),
        };
    }
    if request.op == OP_GRANT_VOLUME {
        return match sessions.volume(sender, letter).grant(
            request.arg0,
            request.arg1 as u32,
            drive.volume_id(),
            drive.epoch(),
        ) {
            Ok(token) => encode_token_response(body, token),
            Err(status) => (status, 0),
        };
    }
    if request.op == OP_RESTORE {
        if let Err(status) = sessions.volume(sender, letter).check(
            request.arg0,
            drive.volume_id(),
            drive.epoch(),
            RIGHT_ADMIN,
        ) {
            return (status, 0);
        }
        return match drive.restore(request.arg1) {
            Ok(generation) => {
                sessions.invalidate_documents(letter);
                encode_u64_response(body, generation)
            }
            Err(error) => (status(error), 0),
        };
    }
    if matches!(
        request.op,
        OP_SNAPSHOT
            | OP_SNAPSHOTS
            | OP_FORGET
            | OP_CHECK
            | OP_REPAIR
            | OP_SCRUB
            | OP_SEAL
            | OP_PRUNE
            | OP_PURGE
            | OP_QUOTA
            | OP_SET_QUOTA
    ) && let Err(status) = sessions.volume(sender, letter).check(
        request.arg0,
        drive.volume_id(),
        drive.epoch(),
        RIGHT_ADMIN,
    ) {
        return (status, 0);
    }
    if request.op == OP_TRANSFER {
        return match sessions.transfer(sender, request.arg1, request.arg0, letter) {
            Ok(token) => encode_token_response(body, token),
            Err(status) => (status, 0),
        };
    }
    let session = sessions.doc(sender, letter);
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
            match cache.read(
                drive,
                letter,
                document,
                None,
                request.arg0,
                &mut body[..length],
            ) {
                Ok(Some(read)) => return (STATUS_OK, read),
                Ok(None) => {}
                Err(error) => return (status(error), 0),
            }
            match drive.read_range(document, request.arg0, &mut body[..length]) {
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
            match cache.read(
                drive,
                letter,
                document,
                Some(revision),
                request.arg0,
                &mut body[..length],
            ) {
                Ok(Some(read)) => return (STATUS_OK, read),
                Ok(None) => {}
                Err(error) => return (status(error), 0),
            }
            match drive.read_range_at(
                document,
                RevisionId::new(revision),
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
            match drive.write_range(document, request.arg0, content) {
                Ok(_) => match drive.document_info(document) {
                    Ok(info) => encode_revision_response(body, info.head),
                    Err(error) => (status(error), 0),
                },
                Err(error) => (status(error), 0),
            }
        }
        OP_STAGE => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let expected = (request.arg0 != 0).then(|| RevisionId::new(request.arg0));
            match drive.stage(document, expected) {
                Ok(staging) => encode_u64_response(body, staging),
                Err(error) => (status(error), 0),
            }
        }
        OP_WRITE_STAGING => match drive.write_staging(request.arg0, request.arg1, request.data) {
            Ok(length) => encode_u64_response(body, length as u64),
            Err(error) => (status(error), 0),
        },
        OP_COMMIT_STAGING => {
            let expected = (request.arg1 != 0).then(|| RevisionId::new(request.arg1));
            match drive.commit_staging(request.arg0, expected) {
                Ok(revision) => encode_revision_response(body, revision),
                Err(error) => (status(error), 0),
            }
        }
        OP_ABORT_STAGING => match drive.abort_staging(request.arg0) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
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
            match cache.read(
                drive,
                letter,
                token.document,
                Some(token.revision.get()),
                request.arg1,
                &mut body[..length],
            ) {
                Ok(Some(read)) => return (STATUS_OK, read),
                Ok(None) => {}
                Err(error) => return (status(error), 0),
            }
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
        OP_FORGET => match drive.forget(request.arg1) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (status(error), 0),
        },
        OP_CHECK => match drive.check() {
            Ok(report) => encode_maintenance_response(body, report),
            Err(error) => (status(error), 0),
        },
        OP_REPAIR => match drive.repair() {
            Ok(report) => encode_maintenance_response(body, report),
            Err(error) => (status(error), 0),
        },
        OP_SCRUB => match drive.scrub() {
            Ok(report) => encode_maintenance_response(body, report),
            Err(error) => (status(error), 0),
        },
        OP_SEAL => {
            let Some(revision) = request
                .data
                .get(..8)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u64::from_le_bytes)
            else {
                return (STATUS_INVALID, 0);
            };
            match drive.seal(RevisionId::new(revision)) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_PRUNE => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.prune(document, request.arg1) {
                Ok(report) => encode_maintenance_response(body, report),
                Err(error) => (status(error), 0),
            }
        }
        OP_PURGE => {
            let Some(document) = doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.purge(document) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
        OP_QUOTA => {
            let quota = drive.quota();
            encode_quota_response(
                body,
                hk_proto::holy::QuotaView {
                    documents: quota.documents,
                    bytes: quota.bytes,
                    revisions: quota.revisions,
                    snapshots: quota.snapshots,
                    tags: quota.tags,
                },
            )
        }
        OP_SET_QUOTA => {
            let Some(view) = decode_quota(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let quota = Quota {
                documents: view.documents,
                bytes: view.bytes,
                revisions: view.revisions,
                snapshots: view.snapshots,
                tags: view.tags,
            };
            match drive.set_quota(quota) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (status(error), 0),
            }
        }
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

fn encode_quota_response(body: &mut [u8], view: hk_proto::holy::QuotaView) -> (u8, usize) {
    match encode_quota(body, view) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
}

fn encode_maintenance_response(
    body: &mut [u8],
    report: hk_fs_holyfs::MaintenanceReport,
) -> (u8, usize) {
    let view = MaintenanceView {
        documents: report.documents,
        revisions: report.revisions,
        collections: report.collections,
        entries: report.entries,
        tags: report.tags,
        snapshots: report.snapshots,
        content_bytes: report.content_bytes,
        leaked_blocks: report.leaked_blocks,
        repaired_blocks: report.repaired_blocks,
    };
    match encode_maintenance(body, view) {
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

fn encode_drives(body: &mut [u8], drives: alloc::vec::Vec<VolumeInfo>) -> (u8, usize) {
    let mut length = 0;
    for info in drives {
        let view = DriveView {
            letter: info.letter,
            volume: VolumeView {
                volume: info.volume.get(),
                generation: info.generation,
                epoch: info.epoch,
                root: WireDocId::new(info.root.get()),
            },
        };
        let Some(end) = encode_drive(body, length, view) else {
            return (STATUS_ERROR, 0);
        };
        length = end;
    }
    (STATUS_OK, length)
}

fn encode_volume_response(body: &mut [u8], view: VolumeView) -> (u8, usize) {
    match encode_volume(body, view) {
        Some(length) => (STATUS_OK, length),
        None => (STATUS_ERROR, 0),
    }
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
        HolyError::Stale => STATUS_STALE,
        HolyError::Damaged => STATUS_DAMAGED,
        HolyError::ReadOnly => STATUS_READ_ONLY,
        _ => STATUS_ERROR,
    }
}
