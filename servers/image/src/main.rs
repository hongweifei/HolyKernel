//! Image Server：接收 Loader 的段注册，按缺页从模块文件复制代码页。
//!
//! 单一接收线程负责所有 Provider 调用与回复；后台 worker 只预读模块页，
//! 避免多线程直接 `recv`/`reply` 破坏调用-回复配对。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::collections::BTreeMap;
#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "none")]
use hk_sync::Mutex;
#[cfg(target_os = "none")]
use hk_syscall::{IMAGE_REGISTER, IMAGE_RESTART};
#[cfg(target_os = "none")]
use hk_user_api::{
    PROVIDER_ERROR, PROVIDER_FAULT, PROVIDER_PROVIDE, boot_cmdline, capacity_alloc, exit,
    frame_write, message_words, module_read, recv_data_handle, reply, reply_handle, sleep_ticks,
    thread_create, write,
};

hk_user_api::server_heap!(2 * 1024 * 1024);
/// 页长度。
#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// 预读 worker 数。
#[cfg(target_os = "none")]
const WORKERS: usize = 3;
/// 每个 worker 栈大小。
#[cfg(target_os = "none")]
const STACK_SIZE: usize = 32 * 1024;
/// 预读缓存页数上限。
#[cfg(target_os = "none")]
const CACHE_PAGES: usize = 256;

/// 一个段来源。
#[cfg(target_os = "none")]
#[derive(Clone, Copy, Default)]
struct Segment {
    object: u64,
    module: u64,
    file_offset: u64,
    file_len: u64,
}

#[cfg(target_os = "none")]
static PENDING: Mutex<Vec<Segment>> = Mutex::new(Vec::new());
#[cfg(target_os = "none")]
static CACHE: Mutex<BTreeMap<(u64, u64, u64), Vec<u8>>> = Mutex::new(BTreeMap::new());
#[cfg(target_os = "none")]
static CACHED_PAGES: AtomicUsize = AtomicUsize::new(0);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, capacity: u64) -> ! {
    write(b"hk-server-image: ready\n");
    let mut line = [0_u8; 64];
    let restart_test =
        boot_cmdline(&mut line).is_some_and(|cmdline| cmdline.contains("image.restart-test"));
    let mut stacks: Vec<Vec<u8>> = Vec::new();
    if !restart_test {
        for _ in 0..WORKERS {
            let mut stack = vec![0_u8; STACK_SIZE];
            let top = stack.as_mut_ptr() as u64 + STACK_SIZE as u64 - 8;
            if thread_create(prefetch as *const () as u64, top, 0, 0) != u64::MAX {
                stacks.push(stack);
            }
        }
    }
    let _stacks = stacks;

    let mut segments: Vec<Segment> = Vec::new();
    let mut buffer = [0_u8; 8];
    loop {
        let (label, _word, sender, _length, _handle) = recv_data_handle(endpoint, &mut buffer, 0);
        if label == IMAGE_REGISTER && sender == 0 {
            if let Some(words) = message_words() {
                let segment = Segment {
                    object: words[0],
                    module: words[1],
                    file_offset: words[2],
                    file_len: words[3],
                };
                if let Some(slot) = segments
                    .iter_mut()
                    .find(|slot| slot.object == segment.object)
                {
                    *slot = segment;
                } else {
                    segments.push(segment);
                }
                PENDING.lock().push(segment);
            }
        } else if label == PROVIDER_FAULT {
            serve_fault(capacity, &segments);
        } else if label == IMAGE_RESTART && restart_test {
            exit(1);
        }
    }
}

/// 预读模块页到缓存；主线程仍负责回复。
#[cfg(target_os = "none")]
extern "C" fn prefetch(_argument: u64) -> ! {
    loop {
        let segment = PENDING.lock().pop();
        let Some(segment) = segment else {
            // 注册消息已全部投递后 worker 退出，避免空闲预热线程阻止系统收尾。
            let mut idle = 0;
            while idle < 32 && PENDING.lock().is_empty() {
                let _ = sleep_ticks(1);
                idle += 1;
            }
            if PENDING.lock().is_empty() {
                exit(0)
            }
            continue;
        };
        let pages = segment.file_len.div_ceil(PAGE as u64);
        for page in 0..pages {
            if CACHED_PAGES.load(Ordering::Acquire) >= CACHE_PAGES {
                break;
            }
            let offset = page * PAGE as u64;
            let length = ((segment.file_len - offset) as usize).min(PAGE);
            let mut bytes = vec![0_u8; PAGE];
            let read = module_read(
                segment.module,
                segment.file_offset.saturating_add(offset),
                &mut bytes[..length],
            );
            if read != length as u64 {
                break;
            }
            let mut cache = CACHE.lock();
            let key = (segment.module, segment.file_offset, offset);
            if cache.insert(key, bytes).is_none() {
                CACHED_PAGES.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

/// 提供一页模块内容；优先使用预读缓存。
#[cfg(target_os = "none")]
fn serve_fault(capacity: u64, segments: &[Segment]) {
    let Some(words) = message_words() else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    let Some(segment) = segments.iter().find(|segment| segment.object == words[4]) else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    let offset = words[0];
    let length = (words[1] as usize).min(PAGE);
    let key = (segment.module, segment.file_offset, offset);
    let cached = CACHE.lock().remove(&key);
    let mut page = if let Some(bytes) = cached {
        if bytes.len() != PAGE {
            let _ = reply(PROVIDER_ERROR, 0);
            return;
        }
        bytes
    } else {
        let mut page = vec![0_u8; PAGE];
        let available = segment.file_len.saturating_sub(offset);
        let file_bytes = usize::try_from(available).unwrap_or(usize::MAX).min(length);
        if file_bytes > 0 {
            let read = module_read(
                segment.module,
                segment.file_offset.saturating_add(offset),
                &mut page[..file_bytes],
            );
            if read != file_bytes as u64 {
                let _ = reply(PROVIDER_ERROR, 0);
                return;
            }
        }
        page
    };
    page.truncate(length);
    page.resize(PAGE, 0);

    let Some(frame) = capacity_alloc(capacity, PAGE) else {
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    };
    if frame_write(frame, 0, &page) != 0 {
        let _ = hk_user_api::close_handle(frame);
        let _ = reply(PROVIDER_ERROR, 0);
        return;
    }
    let _ = reply_handle(PROVIDER_PROVIDE, 0, frame);
}

hk_user_api::server_boilerplate!();
