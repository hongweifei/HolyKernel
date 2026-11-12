//! 原生 Holy 文档请求：DocPath、DocId、偏移读写与句柄令牌。

#![cfg(target_os = "none")]

use hk_fs::HandleRights;
use hk_fs_holyfs::{MaintenanceReport, Quota, RevisionId};
use hk_proto::holy::{
    DocId as WireDocId, DriveView, MaintenanceView, OP_ABORT_STAGING, OP_ACQUIRE,
    OP_ACQUIRE_VOLUME, OP_ATTACH_DRIVE, OP_CHECK, OP_COMMIT_STAGING, OP_CREATE, OP_CREATE_TAG,
    OP_DETACH_DRIVE, OP_DOCUMENT_ID, OP_DOCUMENT_TOKEN, OP_DRIVES, OP_DROP_TAG, OP_ENUMERATE_ID,
    OP_FORGET, OP_GRANT, OP_GRANT_VOLUME, OP_PRUNE, OP_PURGE, OP_QUOTA, OP_READ, OP_READ_ID,
    OP_READ_REVISION, OP_READ_TOKEN, OP_RELEASE, OP_RELEASE_VOLUME, OP_REPAIR, OP_RESOLVE,
    OP_RESTORE, OP_REVISIONS_ID, OP_REVOKE, OP_REVOKE_VOLUME, OP_ROOT, OP_SCRUB, OP_SEAL,
    OP_SET_QUOTA, OP_SNAPSHOT, OP_SNAPSHOTS, OP_STAGE, OP_SYNC, OP_TAG, OP_TAG_MEMBERS, OP_TAGS,
    OP_TAGS_OF, OP_TRANSFER, OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND, OP_UNTAG, OP_VOLUME,
    OP_WRITE_ID, OP_WRITE_STAGING, OP_WRITE_TOKEN, QuotaView, Request, TOKEN_RIGHT_ADMIN,
    TOKEN_RIGHT_INSPECT, TOKEN_RIGHT_READ, TOKEN_RIGHT_WRITE, VolumeView, decode_quota, decode_u64,
    encode_maintenance, encode_quota,
};
use hk_proto::vfs::{STATUS_INVALID, STATUS_NOT_FOUND, STATUS_OK, STATUS_PERMISSION};

use hk_holy_proto::response;

use crate::read_cache::ReadCache;
use crate::sessions::Sessions;
use crate::volumes::{Volumes, parse_drive};

/// 对一个盘符执行一次提交；供 group commit 统一刷写。
pub(crate) fn sync(volumes: &mut Volumes, letter: u8) -> u8 {
    let Some(drive) = volumes.get_mut(letter) else {
        return STATUS_NOT_FOUND;
    };
    match drive.sync() {
        Ok(()) => STATUS_OK,
        Err(error) => response::status(error),
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
        return response::drives(
            body,
            volumes.list().into_iter().map(|info| DriveView {
                letter: info.letter,
                volume: VolumeView {
                    volume: info.volume.get(),
                    generation: info.generation,
                    epoch: info.epoch,
                    root: WireDocId::new(info.root.get()),
                },
            }),
        );
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
        if let Err(error) = sessions.volume(sender, auth).check(
            request.arg0,
            volume_id,
            epoch,
            HandleRights::from_bits(TOKEN_RIGHT_ADMIN),
        ) {
            return (response::status(error), 0);
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
        return response::volume(body, view);
    }
    if request.op == OP_ACQUIRE_VOLUME {
        let token = sessions.volume(sender, letter).acquire(
            drive.volume_id(),
            drive.epoch(),
            HandleRights::from_bits(request.arg0 as u32),
        );
        return response::token(body, token);
    }
    if request.op == OP_RELEASE_VOLUME || request.op == OP_REVOKE_VOLUME {
        return match sessions.volume(sender, letter).release(
            request.arg0,
            drive.volume_id(),
            drive.epoch(),
        ) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        };
    }
    if request.op == OP_GRANT_VOLUME {
        return match sessions.volume(sender, letter).grant(
            request.arg0,
            HandleRights::from_bits(request.arg1 as u32),
            drive.volume_id(),
            drive.epoch(),
        ) {
            Ok(token) => response::token(body, token),
            Err(error) => (response::status(error), 0),
        };
    }
    if request.op == OP_RESTORE {
        if let Err(error) = sessions.volume(sender, letter).check(
            request.arg0,
            drive.volume_id(),
            drive.epoch(),
            HandleRights::from_bits(TOKEN_RIGHT_ADMIN),
        ) {
            return (response::status(error), 0);
        }
        return match drive.restore(request.arg1) {
            Ok(generation) => {
                sessions.invalidate_documents(letter);
                response::number_u64(body, generation)
            }
            Err(error) => (response::status(error), 0),
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
    ) && let Err(error) = sessions.volume(sender, letter).check(
        request.arg0,
        drive.volume_id(),
        drive.epoch(),
        HandleRights::from_bits(TOKEN_RIGHT_ADMIN),
    ) {
        return (response::status(error), 0);
    }
    if request.op == OP_TRANSFER {
        return match sessions.transfer(sender, request.arg1, request.arg0, letter) {
            Ok(token) => response::token(body, token),
            Err(error) => (response::status(error), 0),
        };
    }
    let session = sessions.doc(sender, letter);
    match request.op {
        OP_READ => match drive.read_document(request.path) {
            Ok(content) => response::bytes(body, &content),
            Err(error) => (response::status(error), 0),
        },
        OP_ROOT => response::doc(body, drive.root()),
        OP_RESOLVE => {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.lookup(collection, request.path) {
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
            match drive.create(collection, request.path, kind) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_UNBIND => {
            let Some(collection) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.unlink(collection, request.path) {
                Ok(document) => response::doc(body, document),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_ENUMERATE_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.enumerate_document(document) {
                Ok(entries) => response::entries(body, &entries),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_DOCUMENT_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.document_info(document) {
                Ok(info) => response::info(body, info),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_READ_ID => {
            let Some(document) = response::doc_from(request.data) else {
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
                Err(error) => return (response::status(error), 0),
            }
            match drive.read_range(document, request.arg0, &mut body[..length]) {
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
                Err(error) => return (response::status(error), 0),
            }
            match drive.read_range_at(
                document,
                RevisionId::new(revision),
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
            match drive.write_range(document, request.arg0, content) {
                Ok(_) => match drive.document_info(document) {
                    Ok(info) => response::revision(body, info.head),
                    Err(error) => (response::status(error), 0),
                },
                Err(error) => (response::status(error), 0),
            }
        }
        OP_STAGE => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            let expected = (request.arg0 != 0).then(|| RevisionId::new(request.arg0));
            match drive.stage(document, expected) {
                Ok(staging) => response::number_u64(body, staging),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_WRITE_STAGING => match drive.write_staging(request.arg0, request.arg1, request.data) {
            Ok(length) => response::number_u64(body, length as u64),
            Err(error) => (response::status(error), 0),
        },
        OP_COMMIT_STAGING => {
            let expected = (request.arg1 != 0).then(|| RevisionId::new(request.arg1));
            match drive.commit_staging(request.arg0, expected) {
                Ok(revision) => response::revision(body, revision),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_ABORT_STAGING => match drive.abort_staging(request.arg0) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        },
        OP_TRUNCATE_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.truncate(document, request.arg0) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_REVISIONS_ID => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.revisions_of(document) {
                Ok(revisions) => response::revisions(body, &revisions),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_SYNC => match drive.sync() {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        },
        OP_ACQUIRE => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.document_info(document) {
                Ok(info) => {
                    let token = session.acquire(
                        document,
                        info.head,
                        HandleRights::from_bits(request.arg0 as u32),
                    );
                    response::token(body, token)
                }
                Err(error) => (response::status(error), 0),
            }
        }
        OP_RELEASE | OP_REVOKE => {
            if session.release(request.arg0) {
                (STATUS_OK, 0)
            } else {
                (STATUS_NOT_FOUND, 0)
            }
        }
        OP_GRANT => match session.grant(request.arg0, HandleRights::from_bits(request.arg1 as u32))
        {
            Ok(token) => response::token(body, token),
            Err(error) => (response::status(error), 0),
        },
        OP_READ_TOKEN => {
            let token = match session.get(request.arg0) {
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
                Err(error) => return (response::status(error), 0),
            }
            match drive.read_range_at(
                token.document,
                token.revision,
                request.arg1,
                &mut body[..length],
            ) {
                Ok(read) => (STATUS_OK, read),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_WRITE_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(error) => return (response::status(error), 0),
            };
            if token.rights.bits() & TOKEN_RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match drive.write_range(token.document, request.arg1, request.data) {
                Ok(_) => match drive.document_info(token.document) {
                    Ok(info) => {
                        session.set_revision(request.arg0, info.head);
                        response::revision(body, info.head)
                    }
                    Err(error) => (response::status(error), 0),
                },
                Err(error) => (response::status(error), 0),
            }
        }
        OP_TRUNCATE_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(error) => return (response::status(error), 0),
            };
            if token.rights.bits() & TOKEN_RIGHT_WRITE == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match drive.truncate(token.document, request.arg1) {
                Ok(()) => match drive.document_info(token.document) {
                    Ok(info) => {
                        session.set_revision(request.arg0, info.head);
                        (STATUS_OK, 0)
                    }
                    Err(error) => (response::status(error), 0),
                },
                Err(error) => (response::status(error), 0),
            }
        }
        OP_DOCUMENT_TOKEN => {
            let token = match session.get(request.arg0) {
                Ok(token) => token,
                Err(error) => return (response::status(error), 0),
            };
            if token.rights.bits() & TOKEN_RIGHT_INSPECT == 0 {
                return (STATUS_PERMISSION, 0);
            }
            match drive.document_info(token.document) {
                Ok(info) => response::info(body, info),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_SNAPSHOT => match drive.snapshot(request.path) {
            Ok(snapshot) => response::number_u64(body, snapshot),
            Err(error) => (response::status(error), 0),
        },
        OP_SNAPSHOTS => response::snapshots(body, &drive.snapshots()),
        OP_FORGET => match drive.forget(request.arg1) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        },
        OP_CHECK => match drive.check() {
            Ok(report) => encode_maintenance_response(body, report),
            Err(error) => (response::status(error), 0),
        },
        OP_REPAIR => match drive.repair() {
            Ok(report) => encode_maintenance_response(body, report),
            Err(error) => (response::status(error), 0),
        },
        OP_SCRUB => match drive.scrub() {
            Ok(report) => encode_maintenance_response(body, report),
            Err(error) => (response::status(error), 0),
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
                Err(error) => (response::status(error), 0),
            }
        }
        OP_PRUNE => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.prune(document, request.arg1) {
                Ok(report) => encode_maintenance_response(body, report),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_PURGE => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.purge(document) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_QUOTA => {
            let quota = drive.quota();
            encode_quota_response(
                body,
                QuotaView {
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
                Err(error) => (response::status(error), 0),
            }
        }
        OP_TAGS => response::tags(body, &drive.tags()),
        OP_TAG_MEMBERS => match drive.tag_members(request.arg0 as u32) {
            Ok(documents) => response::list_docs(body, &documents),
            Err(error) => (response::status(error), 0),
        },
        OP_CREATE_TAG => match drive.create_tag(request.path) {
            Ok(tag) => response::number_u32(body, tag),
            Err(error) => (response::status(error), 0),
        },
        OP_DROP_TAG => match drive.drop_tag(request.arg0 as u32) {
            Ok(()) => (STATUS_OK, 0),
            Err(error) => (response::status(error), 0),
        },
        OP_TAG => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.tag(document, request.arg0 as u32) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_UNTAG => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.untag(document, request.arg0 as u32) {
                Ok(()) => (STATUS_OK, 0),
                Err(error) => (response::status(error), 0),
            }
        }
        OP_TAGS_OF => {
            let Some(document) = response::doc_from(request.data) else {
                return (STATUS_INVALID, 0);
            };
            match drive.tags_of(document) {
                Ok(tags) => response::list_u32(body, &tags),
                Err(error) => (response::status(error), 0),
            }
        }
        _ => (STATUS_INVALID, 0),
    }
}

fn encode_quota_response(body: &mut [u8], view: QuotaView) -> (u8, usize) {
    response::encoded(encode_quota(body, view))
}

fn encode_maintenance_response(body: &mut [u8], report: MaintenanceReport) -> (u8, usize) {
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
    response::encoded(encode_maintenance(body, view))
}
