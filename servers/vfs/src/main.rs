//! VFS Server：DocId、集合、修订与文档令牌的原生命名空间。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_fs::{Backend, DocId as FsDocId, DocSessions, RevisionId as FsRevisionId};
#[cfg(target_os = "none")]
use hk_fs_remote::RemoteHolyFs;
#[cfg(target_os = "none")]
use hk_holy_service::{Drives, handle};
#[cfg(target_os = "none")]
use hk_proto::holy::{HOLY_REQUEST, HOLY_RESPONSE, decode_drives, decode_request};
#[cfg(target_os = "none")]
use hk_proto::vfs::encode_response;
#[cfg(target_os = "none")]
use hk_proto_client::HolyClient;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, PROVIDER_ERROR, PROVIDER_EVICT, PROVIDER_FAULT, PROVIDER_KIND_REVISION,
    PROVIDER_PROVIDE, PROVIDER_RELEASE, PROVIDER_WRITEBACK, RIGHT_READ, RIGHT_TRANSFER,
    capability_bind_provider, capability_create_lazy, capacity_alloc, close_handle, frame_write,
    grant_capability, handle_id, message_words, recv_data_handle, reply_handle, send_data,
    send_handle, write, yield_now,
};
#[cfg(target_os = "none")]
use hk_vfs::{Devfs, PathObject, Procfs, Tmpfs};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 2 * 1024 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
fn reply(sender: u64, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, status, data) {
        let _ = send_data(sender, HOLY_RESPONSE, &response[..length]);
    }
}

/// Provider 页长度。
#[cfg(target_os = "none")]
const PAGE: usize = 4096;

/// 已映射修订能力的元数据。
#[cfg(target_os = "none")]
struct RevisionRecord {
    capability: u64,
    drive: u8,
    document: FsDocId,
    revision: FsRevisionId,
    size: u64,
    /// 最近一次顺序读取的窗口；用于 readahead。
    cache: Vec<u8>,
    /// 缓存窗口在修订中的起始偏移。
    cache_offset: u64,
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, capacity: u64) -> ! {
    let mut cmdline_buffer = [0_u8; 128];
    let holyfs = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("vfs.fs=holyfs"));
    let mut drives = Drives::new();
    if holyfs {
        for letter in discover(endpoint, "hk-server-holyfs") {
            if let Some(backend) = remote(endpoint, letter, "hk-server-holyfs") {
                insert_remote(&mut drives, letter, backend);
            }
        }
        if drives.is_empty() {
            write(b"hk-server-vfs: no drives\n");
        }
    } else if let Some(backend) = remote(endpoint, b'E', "hk-server-ext4") {
        insert_remote(&mut drives, b'E', backend);
    } else {
        write(b"hk-server-vfs: no ext4 service\n");
    }
    register_local(&mut drives, b'T', PathObject::new(Tmpfs::new()));
    register_local(&mut drives, b'D', PathObject::new(Devfs::new()));
    register_local(&mut drives, b'P', PathObject::new(Procfs::new()));

    let mut sessions = DocSessions::new();
    let mut revisions: Vec<RevisionRecord> = Vec::new();
    let mut request = vec![0_u8; 64 * 1024];
    let mut body = vec![0_u8; 64 * 1024 - 4];
    let mut response = vec![0_u8; 64 * 1024];

    loop {
        let (label, _word, sender, length, frame_handle) =
            recv_data_handle(endpoint, &mut request, 0);
        if label == PROVIDER_RELEASE {
            if let Some(words) = message_words() {
                revisions.retain(|record| record.capability != words[0]);
            }
            continue;
        }
        if label == PROVIDER_FAULT {
            serve_fault(&mut drives, &mut revisions, capacity);
            continue;
        }
        if label == PROVIDER_EVICT || label == PROVIDER_WRITEBACK {
            // 修订页只读且由 Backend 持有，没有可回写内容。
            continue;
        }
        if label != HOLY_REQUEST || frame_handle != 0 {
            continue;
        }
        if length >= 24
            && let Some(request) = decode_request(&request[..length.min(request.len())])
            && request.op == hk_proto::holy::OP_MAP_REVISION
        {
            map_revision(
                &mut drives,
                &mut revisions,
                endpoint,
                sender,
                &request,
                &mut response,
            );
            continue;
        }
        let Some(request) = decode_request(&request[..length.min(request.len())]) else {
            reply(sender, hk_proto::vfs::STATUS_INVALID, &[], &mut response);
            continue;
        };
        let (status, written) = handle(&mut drives, &mut sessions, sender, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

#[cfg(target_os = "none")]
fn insert_remote(drives: &mut Drives, letter: u8, backend: RemoteHolyFs) {
    let volume = backend.volume();
    let root = backend.root();
    if drives.insert(letter, Box::new(backend), volume, root) {
        log_drive(letter);
    }
}

#[cfg(target_os = "none")]
fn register_local<P: hk_fs::Backend + 'static>(drives: &mut Drives, letter: u8, backend: P) {
    if drives.insert_local(letter, backend) {
        log_drive(letter);
    }
}

#[cfg(target_os = "none")]
fn log_drive(letter: u8) {
    write(&[letter, b':', b'\n']);
}

#[cfg(target_os = "none")]
fn discover(endpoint: u64, service: &'static str) -> Vec<u8> {
    for _ in 0..4096 {
        if hk_server_api::service_endpoint(service) == 0 {
            let _ = yield_now();
            continue;
        }
        let mut probe = HolyClient::new_with_service(endpoint, service);
        if let Ok(bytes) = probe.drives() {
            return decode_drives(bytes).map(|drive| drive.letter).collect();
        }
        let _ = yield_now();
    }
    Vec::new()
}

#[cfg(target_os = "none")]
fn remote(endpoint: u64, letter: u8, service: &'static str) -> Option<RemoteHolyFs> {
    for _ in 0..4096 {
        if let Ok(backend) = RemoteHolyFs::new(endpoint, letter, service) {
            return Some(backend);
        }
        let _ = yield_now();
    }
    None
}

/// 把请求中的固定修订暴露为只读懒能力。
#[cfg(target_os = "none")]
fn map_revision(
    drives: &mut Drives,
    revisions: &mut Vec<RevisionRecord>,
    endpoint: u64,
    sender: u64,
    request: &hk_proto::holy::Request<'_>,
    response: &mut [u8],
) {
    let Some(document) = request
        .data
        .get(..16)
        .and_then(|bytes| bytes.try_into().ok().map(FsDocId::from_le_bytes))
    else {
        reply(sender, hk_proto::vfs::STATUS_INVALID, &[], response);
        return;
    };
    let Some(revision) = request
        .data
        .get(16..24)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
    else {
        reply(sender, hk_proto::vfs::STATUS_INVALID, &[], response);
        return;
    };
    let revision = FsRevisionId::new(revision);
    let Some(drive) = drives.get_mut(request.drive) else {
        reply(sender, hk_proto::vfs::STATUS_NOT_FOUND, &[], response);
        return;
    };
    let Ok(chain) = drive.backend.revisions(document) else {
        reply(sender, hk_proto::vfs::STATUS_NOT_FOUND, &[], response);
        return;
    };
    let Some(size) = chain
        .iter()
        .find(|info| info.id == revision)
        .map(|info| info.size)
    else {
        reply(sender, hk_proto::vfs::STATUS_NOT_FOUND, &[], response);
        return;
    };
    if size == 0 {
        reply(sender, hk_proto::vfs::STATUS_INVALID, &[], response);
        return;
    }
    // 同一修订只保留一个能力对象：后续映射者拿到同一对象的只读句柄，缺页装入的
    // 页由所有映射共享。对象最后一位持有者关闭后内核发 PROVIDER_RELEASE，记录
    // 随之移除，不需要 VFS 自己持有句柄。
    if let Some(index) = revisions.iter().position(|record| {
        record.drive == request.drive && record.document == document && record.revision == revision
    }) {
        let capability = revisions[index].capability;
        if let Some(handle) = grant_capability(capability, RIGHT_READ | RIGHT_TRANSFER) {
            write(b"hk-server-vfs: revision shared\n");
            if send_handle(sender, HOLY_RESPONSE, size, handle) != 0 {
                let _ = close_handle(handle);
                reply(sender, hk_proto::vfs::STATUS_ERROR, &[], response);
            }
            return;
        }
        // 对象已经析构但释放通知还没被消费：清掉陈旧记录，重新建立。
        revisions.remove(index);
    }
    let Some(capability) = capability_create_lazy(CapabilityKind::Revision, size as usize) else {
        reply(sender, hk_proto::vfs::STATUS_ERROR, &[], response);
        return;
    };
    if capability_bind_provider(capability, endpoint, PROVIDER_KIND_REVISION) != 0 {
        let _ = close_handle(capability);
        reply(sender, hk_proto::vfs::STATUS_ERROR, &[], response);
        return;
    }
    let Some(capability_id) = handle_id(capability) else {
        let _ = close_handle(capability);
        reply(sender, hk_proto::vfs::STATUS_ERROR, &[], response);
        return;
    };
    revisions.push(RevisionRecord {
        capability: capability_id,
        drive: request.drive,
        document,
        revision,
        size,
        cache: Vec::new(),
        cache_offset: 0,
    });
    if send_handle(sender, HOLY_RESPONSE, size, capability) != 0 {
        let _ = close_handle(capability);
    }
}

/// 读取修订内容填充 Provider 请求的页。
/// 单次 readahead 的页数。
#[cfg(target_os = "none")]
const READAHEAD_PAGES: u64 = 4;
/// 全部修订窗口缓存上限；超出时清掉其他修订的窗口（不可变内容可重读）。
#[cfg(target_os = "none")]
const CACHE_BUDGET: usize = 1024 * 1024;

#[cfg(target_os = "none")]
fn serve_fault(drives: &mut Drives, revisions: &mut [RevisionRecord], capacity: u64) {
    let Some(words) = message_words() else {
        let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
        return;
    };
    if words[3] != 0 {
        let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
        return;
    }
    let Some(index) = revisions
        .iter()
        .position(|record| record.capability == words[4])
    else {
        let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
        return;
    };
    let offset = words[0];
    let request = (words[1] as usize).min(PAGE);
    let (drive, document, revision, size) = {
        let record = &revisions[index];
        (record.drive, record.document, record.revision, record.size)
    };
    let length = request.min(size.saturating_sub(offset) as usize);
    let mut buffer = [0_u8; PAGE];
    if length > 0 {
        let window = PAGE as u64 * READAHEAD_PAGES;
        let window_start = offset / window * window;
        let cache_end = revisions[index].cache_offset + revisions[index].cache.len() as u64;
        if offset < revisions[index].cache_offset || offset + length as u64 > cache_end {
            let read_len = size.saturating_sub(window_start).min(window) as usize;
            let mut cache = vec![0_u8; read_len];
            let Some(drive) = drives.get_mut(drive) else {
                let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
                return;
            };
            match drive
                .backend
                .read(document, Some(revision), window_start, &mut cache)
            {
                Ok(written) => {
                    let written = written.min(cache.len());
                    for byte in &mut cache[written..] {
                        *byte = 0;
                    }
                }
                Err(_) => {
                    let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
                    return;
                }
            }
            let total: usize = revisions
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, record)| record.cache.len())
                .sum();
            if total.saturating_add(cache.len()) > CACHE_BUDGET {
                for (other, record) in revisions.iter_mut().enumerate() {
                    if other != index {
                        record.cache.clear();
                        record.cache_offset = 0;
                    }
                }
            }
            revisions[index].cache = cache;
            revisions[index].cache_offset = window_start;
        }
        let start = (offset - revisions[index].cache_offset) as usize;
        let available = revisions[index].cache.len().saturating_sub(start);
        let copied = length.min(available);
        buffer[..copied].copy_from_slice(&revisions[index].cache[start..start + copied]);
        for byte in &mut buffer[copied..length] {
            *byte = 0;
        }
    }
    let Some(frame) = capacity_alloc(capacity, PAGE) else {
        let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
        return;
    };
    if frame_write(frame, 0, &buffer) != 0 {
        let _ = close_handle(frame);
        let _ = hk_server_api::reply(PROVIDER_ERROR, 0);
        return;
    }
    let _ = reply_handle(PROVIDER_PROVIDE, 0, frame);
}

hk_server_api::server_boilerplate!();
