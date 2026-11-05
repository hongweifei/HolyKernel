//! HolyDoc 自检：令牌、标签、标签页与脚本化导航。

use alloc::boxed::Box;

use hk_server_api::display::DISPLAY_SHUTDOWN;
use hk_server_api::holy::{
    DocumentKind, HolyClient, RIGHT_ADMIN, RIGHT_READ, RIGHT_WRITE, STATUS_NOT_FOUND,
    STATUS_PERMISSION, STATUS_STALE, decode_doc_ids, decode_snapshots, decode_tags,
};
use hk_server_api::{exit, send, write, write_u64};
use lovyangfx_fonts::Font;

use crate::document::{DocSet, DocView, TAGS};
use crate::drive;
use crate::window::apply;

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

/// 执行脚本化的导航、滚动与退出。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    view: &mut DocView,
    docs: &mut DocSet,
    font: &Font<'static>,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    window: u64,
    loaded: usize,
) -> ! {
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
    if let Some((x, y, target)) = view.first_link() {
        write(b"hk-server-holydoc: click ");
        write(target.as_bytes());
        write(b"\n");
        if view.link_at(x, y).is_some() && view.open(docs, &target) {
            apply(view, font, pixels, endpoint, display, window);
        }
    }
    if loaded > 0
        && let Some(target) = docs.resolve("H:/System/Overview")
        && view.open(docs, &target)
    {
        apply(view, font, pixels, endpoint, display, window);
    }
    view.scroll_by(5);
    apply(view, font, pixels, endpoint, display, window);
    write(b"hk-server-holydoc: scroll ");
    write_u64(view.scroll as u64);
    write(b"\n");
    if view.back(docs) {
        apply(view, font, pixels, endpoint, display, window);
    }
    if view.advance(docs) {
        apply(view, font, pixels, endpoint, display, window);
    }
    if view.select_link(true) {
        apply(view, font, pixels, endpoint, display, window);
    }
    if view.open_selected(docs) {
        apply(view, font, pixels, endpoint, display, window);
    }
    write(b"hk-server-holydoc: done\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}
