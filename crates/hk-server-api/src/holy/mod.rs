//! Holy 卷原生文档协议：DocPath、DocId、偏移读写与句柄令牌。
//!
//! DocPath 是集合项名字的查找视图，DocId 承载文档身份；内容按偏移读写，
//! 句柄令牌按身份携带权利。

mod client;
mod codec;
mod ids;
mod wire;

pub use wire::{Request, decode_request, encode_request};

pub use crate::vfs::{STATUS_NOT_FOUND, STATUS_PERMISSION, STATUS_STALE};
pub use client::HolyClient;
pub use codec::{
    DocIds, DocumentView, DriveView, Drives, Entries, Entry, MaintenanceView, RevisionView,
    Revisions, SnapshotView, Snapshots, TagView, Tags, VolumeView, decode_doc_id, decode_doc_ids,
    decode_document, decode_drives, decode_entries, decode_maintenance, decode_revision_id,
    decode_revisions, decode_snapshots, decode_tags, decode_token, decode_u32, decode_u64,
    decode_volume, encode_doc_id, encode_document, encode_drive, encode_entry, encode_maintenance,
    encode_revision, encode_revision_id, encode_snapshot, encode_tag, encode_token, encode_u32,
    encode_u64, encode_volume, kind_code,
};
pub use ids::{DocId, DocumentKind, RevisionId};
pub use wire::{
    DRIVE_SERVICE, HOLY_REQUEST, HOLY_RESPONSE, KIND_COLLECTION, KIND_CONTENT, OP_ACQUIRE,
    OP_ACQUIRE_VOLUME, OP_ATTACH_DRIVE, OP_CHECK, OP_CREATE, OP_CREATE_TAG, OP_DETACH_DRIVE,
    OP_DOCUMENT, OP_DOCUMENT_ID, OP_DOCUMENT_TOKEN, OP_DRIVES, OP_DROP_TAG, OP_ENUMERATE,
    OP_ENUMERATE_ID, OP_FORGET, OP_GRANT, OP_GRANT_VOLUME, OP_PRUNE, OP_PURGE, OP_READ, OP_READ_ID,
    OP_READ_REVISION, OP_READ_TOKEN, OP_RELEASE, OP_RELEASE_VOLUME, OP_REPAIR, OP_RESOLVE,
    OP_RESTORE, OP_REVISIONS, OP_REVISIONS_ID, OP_REVOKE, OP_REVOKE_VOLUME, OP_ROOT, OP_SCRUB,
    OP_SEAL, OP_SNAPSHOT, OP_SNAPSHOTS, OP_SYNC, OP_TAG, OP_TAG_MEMBERS, OP_TAGS, OP_TAGS_OF,
    OP_TRANSFER, OP_TRUNCATE_ID, OP_TRUNCATE_TOKEN, OP_UNBIND, OP_UNTAG, OP_VOLUME, OP_WRITE_ID,
    OP_WRITE_TOKEN, RIGHT_ADMIN, RIGHT_BIND, RIGHT_INSPECT, RIGHT_READ, RIGHT_TRANSFER,
    RIGHT_WRITE,
};
