//! HolyDoc 自检：令牌、标签、标签页与脚本化导航。

use alloc::boxed::Box;
use alloc::vec::Vec;

use hk_proto::display::DISPLAY_SHUTDOWN;
use hk_proto::holy::{
    DocumentKind, HolyClient, QuotaView, RIGHT_ADMIN, RIGHT_READ, RIGHT_WRITE, STATUS_NOT_FOUND,
    STATUS_PERMISSION, STATUS_STALE, decode_doc_ids, decode_drives, decode_snapshots, decode_tags,
};
use hk_server_api::{exit, send, write, write_u64};

use crate::document::{DocSet, DocView, TAGS};
use crate::drive;
use crate::window::apply;

/// 验证盘符注册、卷身份隔离与切盘读写。
pub(crate) fn verify_drives(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(bytes) = client.drives() else {
        return false;
    };
    let drives: Vec<_> = decode_drives(bytes).collect();
    let Some(h) = drives.iter().find(|drive| drive.letter == b'H') else {
        return false;
    };
    let Some(i) = drives.iter().find(|drive| drive.letter == b'I') else {
        return false;
    };
    if h.volume.volume == i.volume.volume || client.select_drive("I").is_err() {
        return false;
    }
    let Ok(root) = client.root() else {
        return false;
    };
    let Ok(document) = client.create(root, "DriveProbe", DocumentKind::Content) else {
        return false;
    };
    if client.write_id(document, 0, b"volume-i").is_err() {
        return false;
    }
    let mut buffer = [0_u8; 16];
    let content_ok = client
        .read_id(document, 0, &mut buffer)
        .is_ok_and(|length| &buffer[..length] == b"volume-i");
    if client.select_drive("H").is_err() {
        return false;
    }
    let h_before = {
        let Ok(root) = client.root() else {
            return false;
        };
        let Ok(document) = client.resolve(root, "Welcome") else {
            return false;
        };
        let mut data = [0_u8; 8];
        client
            .read_id(document, 0, &mut data)
            .map(|length| (length, data))
            .unwrap_or((0, data))
    };
    let Ok(admin) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let attached = client.attach_drive(admin, "J", "hk-server-block").is_ok();
    let listed = client
        .drives()
        .is_ok_and(|bytes| decode_drives(bytes).any(|drive| drive.letter == b'J'));
    let mounted = client.select_drive("J").is_ok() && client.root().is_ok();
    let detached = client.select_drive("H").is_ok() && client.detach_drive(admin, "J").is_ok();
    let gone = client
        .drives()
        .is_ok_and(|bytes| !decode_drives(bytes).any(|drive| drive.letter == b'J'));
    let h_after = {
        let Ok(root) = client.root() else {
            return false;
        };
        let Ok(document) = client.resolve(root, "Welcome") else {
            return false;
        };
        let mut data = [0_u8; 8];
        client
            .read_id(document, 0, &mut data)
            .map(|length| (length, data))
            .unwrap_or((0, data))
    };
    let released = client.release_volume(admin).is_ok();
    content_ok
        && attached
        && listed
        && mounted
        && detached
        && gone
        && h_before == h_after
        && released
}

/// 验证快照固定、恢复与释放。
pub(crate) fn verify_snapshot(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(root) = client.root() else {
        return false;
    };
    let Ok(document) = client.create(root, "SnapshotProbe", DocumentKind::Content) else {
        return false;
    };
    if client.write_id(document, 0, b"before").is_err() {
        return false;
    }
    let Ok(token) = client.acquire(document, RIGHT_READ) else {
        return false;
    };
    let Ok(volume) = client.volume() else {
        return false;
    };
    let Ok(admin) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let Ok(snapshot) = client.snapshot(admin, "smoke") else {
        return false;
    };
    if client.write_id(document, 0, b"after!").is_err() {
        return false;
    }
    let mut buffer = [0_u8; 16];
    let changed = client
        .read_id(document, 0, &mut buffer)
        .is_ok_and(|length| &buffer[..length] == b"after!");
    let Ok(generation) = client.restore(admin, snapshot) else {
        return false;
    };
    let before = client
        .read_id(document, 0, &mut buffer)
        .is_ok_and(|length| &buffer[..length] == b"before");
    let stale = client.read_token(token, 0, &mut buffer) == Err(STATUS_STALE);
    let stale_volume = client.snapshots(admin) == Err(STATUS_STALE);
    let epoch_advanced = client.volume().is_ok_and(|view| view.epoch > volume.epoch);
    let Ok(fresh) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let listed = client.snapshots(fresh).is_ok_and(|bytes| {
        decode_snapshots(bytes)
            .any(|entry| entry.id == snapshot && entry.name == "smoke" && entry.documents > 0)
    });
    let forgotten = client.forget(fresh, snapshot).is_ok();
    let gone = client
        .snapshots(fresh)
        .is_ok_and(|bytes| !decode_snapshots(bytes).any(|entry| entry.id == snapshot));
    let released = client.release_volume(fresh).is_ok();
    generation > 0
        && changed
        && before
        && stale
        && stale_volume
        && epoch_advanced
        && listed
        && forgotten
        && gone
        && released
}

/// 验证令牌收窄、撤销与固定修订读取。
pub(crate) fn verify_tokens(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(root) = client.root() else {
        return false;
    };
    let Ok(document) = client.resolve(root, "Welcome") else {
        return false;
    };
    let Ok(token) = client.acquire(document, RIGHT_READ) else {
        return false;
    };
    let Ok(child) = client.grant(token, RIGHT_READ) else {
        let _ = client.release(token);
        return false;
    };
    let mut buffer = [0_u8; 256];
    let Ok(length) = client.read_token(child, 0, &mut buffer) else {
        let _ = client.release(token);
        return false;
    };
    let read_ok = buffer[..length].starts_with(b"@title");
    let grant_rejected = client.grant(token, RIGHT_WRITE) == Err(STATUS_PERMISSION);
    let revoked = client.revoke(child).is_ok();
    let read_rejected = client.read_token(child, 0, &mut buffer) == Err(STATUS_NOT_FOUND);
    let _ = client.release(token);
    read_ok && grant_rejected && revoked && read_rejected
}

/// 验证标签创建、成员关系、标签页与删除。
pub(crate) fn verify_tags(endpoint: u64, docs: &mut DocSet) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(root) = client.root() else {
        return false;
    };
    let Ok(document) = client.resolve(root, "Welcome") else {
        return false;
    };
    let Ok(tag) = client.create_tag("smoke") else {
        return false;
    };
    if client.tag(document, tag).is_err() {
        return false;
    }
    let contains_document = match client.tag_members(tag) {
        Ok(bytes) => decode_doc_ids(bytes).any(|entry| entry == document),
        Err(_) => false,
    };
    let contains_tag = match client.tags_of(document) {
        Ok(bytes) => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .any(|chunk| u32::from_le_bytes(*chunk) == tag),
        Err(_) => false,
    };
    let listed = match client.tags() {
        Ok(bytes) => decode_tags(bytes).any(|entry| entry.id == tag && entry.name == "smoke"),
        Err(_) => false,
    };
    drive::load_tags(&mut client, docs);
    let page_ok = docs
        .page(TAGS)
        .is_some_and(|page| page.body.contains("smoke"));
    let dropped = client.drop_tag(tag).is_ok();
    let gone = client.tag_members(tag) == Err(STATUS_NOT_FOUND);
    contains_document && contains_tag && listed && page_ok && dropped && gone
}

/// 验证在线完整性检查、泄漏回收与位图修复。
pub(crate) fn verify_maintenance(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(admin) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let check = client.check(admin);
    let scrub = client.scrub(admin);
    let repair = client.repair(admin);
    let released = client.release_volume(admin).is_ok();
    check
        .is_ok_and(|report| report.documents > 0 && report.entries > 0 && report.leaked_blocks == 0)
        && scrub.is_ok_and(|report| report.repaired_blocks == 0)
        && repair.is_ok_and(|report| report.repaired_blocks == 0)
        && released
}

/// 验证封存、裁剪、GC 与显式销毁。
pub(crate) fn verify_retention(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(root) = client.root() else {
        return false;
    };
    let Ok(document) = client.create(root, "RetentionProbe", DocumentKind::Content) else {
        return false;
    };
    let Ok(first) = client.write_id(document, 0, b"one") else {
        return false;
    };
    if client.write_id(document, 0, b"two").is_err()
        || client.write_id(document, 0, b"three").is_err()
    {
        return false;
    }
    let Ok(admin) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let sealed = client.seal(admin, first).is_ok();
    let pruned = client
        .prune(admin, document, 1)
        .is_ok_and(|report| report.repaired_blocks > 0);
    let mut buffer = [0_u8; 8];
    let content_ok = client
        .read_id(document, 0, &mut buffer)
        .is_ok_and(|length| &buffer[..length] == b"three");
    let purged = client.purge(admin, document).is_ok()
        && client.resolve(root, "RetentionProbe") == Err(STATUS_NOT_FOUND);
    let clean = client
        .check(admin)
        .is_ok_and(|report| report.leaked_blocks == 0);
    let released = client.release_volume(admin).is_ok();
    sealed && pruned && content_ok && purged && clean && released
}

/// 验证配额查询与设置。
pub(crate) fn verify_quota(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(admin) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let Ok(original) = client.quota(admin) else {
        return false;
    };
    let target = QuotaView {
        documents: 1024,
        bytes: 16 * 1024 * 1024,
        revisions: 4096,
        snapshots: 64,
        tags: 64,
    };
    let set = client.set_quota(admin, target).is_ok();
    let read = client.quota(admin) == Ok(target);
    let restored = client.set_quota(admin, original).is_ok();
    let released = client.release_volume(admin).is_ok();
    set && read && restored && released
}

/// 验证 staging 流式写、提交与清理。
pub(crate) fn verify_streaming(endpoint: u64) -> bool {
    let mut client = Box::new(HolyClient::new(endpoint));
    let Ok(root) = client.root() else {
        return false;
    };
    let Ok(document) = client.create(root, "StreamProbe", DocumentKind::Content) else {
        return false;
    };
    let Ok(staging) = client.stage(document, None) else {
        return false;
    };
    let written = client.write_staging(staging, 0, b"stream").is_ok();
    let committed = client.commit_staging(staging, None).is_ok();
    let mut buffer = [0_u8; 8];
    let content = client
        .read_id(document, 0, &mut buffer)
        .is_ok_and(|length| &buffer[..length] == b"stream");
    let Ok(admin) = client.acquire_volume(RIGHT_ADMIN) else {
        return false;
    };
    let purged = client.purge(admin, document).is_ok();
    let released = client.release_volume(admin).is_ok();
    written && committed && content && purged && released
}

/// 执行脚本化的导航、滚动与退出。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    view: &mut DocView,
    docs: &mut DocSet,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
    loaded: usize,
) -> ! {
    if verify_drives(endpoint) {
        write(b"hk-server-holydoc: drives ok\n");
    } else {
        write(b"hk-server-holydoc: drives failed\n");
    }
    if verify_snapshot(endpoint) {
        write(b"hk-server-holydoc: snapshot ok\n");
    } else {
        write(b"hk-server-holydoc: snapshot failed\n");
    }
    if verify_tokens(endpoint) {
        write(b"hk-server-holydoc: token ok\n");
    } else {
        write(b"hk-server-holydoc: token failed\n");
    }
    if verify_tags(endpoint, docs) {
        write(b"hk-server-holydoc: tag ok\n");
    } else {
        write(b"hk-server-holydoc: tag failed\n");
    }
    if verify_maintenance(endpoint) {
        write(b"hk-server-holydoc: maintenance ok\n");
    } else {
        write(b"hk-server-holydoc: maintenance failed\n");
    }
    if verify_retention(endpoint) {
        write(b"hk-server-holydoc: retention ok\n");
    } else {
        write(b"hk-server-holydoc: retention failed\n");
    }
    if verify_quota(endpoint) {
        write(b"hk-server-holydoc: quota ok\n");
    } else {
        write(b"hk-server-holydoc: quota failed\n");
    }
    if verify_streaming(endpoint) {
        write(b"hk-server-holydoc: stream ok\n");
    } else {
        write(b"hk-server-holydoc: stream failed\n");
    }
    if let Some((x, y, target)) = view.first_link() {
        write(b"hk-server-holydoc: click ");
        write(target.as_bytes());
        write(b"\n");
        if view.link_at(x, y).is_some() && view.open(docs, &target) {
            apply(view, pixels, endpoint, display, surface, window);
        }
    }
    if loaded > 0
        && let Some(target) = docs.resolve("H:/System/Overview")
        && view.open(docs, &target)
    {
        apply(view, pixels, endpoint, display, surface, window);
    }
    view.scroll_by(5);
    apply(view, pixels, endpoint, display, surface, window);
    write(b"hk-server-holydoc: scroll ");
    write_u64(view.scroll as u64);
    write(b"\n");
    if view.back(docs) {
        apply(view, pixels, endpoint, display, surface, window);
    }
    if view.advance(docs) {
        apply(view, pixels, endpoint, display, surface, window);
    }
    if view.select_link(true) {
        apply(view, pixels, endpoint, display, surface, window);
    }
    if view.open_selected(docs) {
        apply(view, pixels, endpoint, display, surface, window);
    }
    write(b"hk-server-holydoc: done\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}
